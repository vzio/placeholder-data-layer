use std::collections::{BTreeMap, BTreeSet};

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;

use crate::classify::{self, SemanticType};
use crate::dataset::{ColumnData, Value};
use crate::profile::{ColumnProfile, Distribution, LengthStats, NumericProfile};

/// A value's length is judged truncated once it falls below this fraction of
/// the column's observed mean length. Below the generic whitespace and
/// encoding checks, which hold regardless of scale, this is the one detector
/// whose threshold is a judgement call rather than a fact about the shape.
const TRUNCATION_RATIO: f64 = 0.3;

/// How many robust standard deviations from the column's observed median a
/// numeric value has to sit before it is judged out of range.
const OUT_OF_RANGE_SIGMA: f64 = 4.0;

/// Scales a median absolute deviation onto the same footing as a normal
/// distribution's standard deviation (`1 / Φ⁻¹(3/4)`), so [`OUT_OF_RANGE_SIGMA`]
/// means the same thing whichever estimator produced the spread it is
/// compared against.
const MAD_TO_SIGMA: f64 = 1.4826;

/// Case-insensitive, trimmed stand-ins for a missing value that production
/// systems write into a non-null column instead of an actual `NULL`.
const NULL_SENTINELS: &[&str] = &["null", "n/a", "na", "none", "undefined", "nil", "-", "--"];

/// The stand-in for a missing value [`AnomalyKind::corrupt_avoiding`] writes
/// when every entry in [`NULL_SENTINELS`] would reproduce the source. It
/// reads as [`AnomalyKind::UnexpectedNull`] and nothing else, and an empty
/// cell is never compared against the source.
const EMPTY_NULL_SENTINEL: &str = "";

/// Fixed substrings that mark classic UTF-8-decoded-as-Latin-1 corruption on
/// pt-BR text (`café` -> `cafÃ©`). Not exhaustive, but these are the sequences
/// that actually recur in production dumps, which is the bar this detector
/// has to clear.
pub(crate) const MOJIBAKE_MARKERS: &[&str] = &[
    "Ã©", "Ã¡", "Ã¢", "Ã£", "Ã§", "Ã³", "Ã´", "Ãº", "Ã\u{ad}", "Â«", "Â»", "â€™", "â€œ",
];

/// Classes of production corruption the engine can both detect and
/// synthesise. Enum dispatch: adding a kind is a variant plus an arm in
/// [`detect`] and in [`AnomalyKind::corrupt`], never a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    MalformedEmail,
    MalformedDocument,
    MalformedPhone,
    /// An all-uppercase, all-lowercase or digit-bearing name; [`NameShape`] tells them apart.
    MalformedName,
    OutOfRange,
    ImpossibleDate,
    WhitespacePadding,
    EncodingArtifact,
    TruncatedText,
    UnexpectedNull,
}

impl AnomalyKind {
    /// Every kind this build detects and corrupts, used to drive exhaustive
    /// tests across both without either side silently falling behind.
    pub const ALL: &'static [AnomalyKind] = &[
        AnomalyKind::MalformedEmail,
        AnomalyKind::MalformedDocument,
        AnomalyKind::MalformedPhone,
        AnomalyKind::MalformedName,
        AnomalyKind::OutOfRange,
        AnomalyKind::ImpossibleDate,
        AnomalyKind::WhitespacePadding,
        AnomalyKind::EncodingArtifact,
        AnomalyKind::TruncatedText,
        AnomalyKind::UnexpectedNull,
    ];

    /// Manufactures this class of corruption on an already-anonymized value.
    ///
    /// Detected exemplars are production PII and are never replayed: this
    /// always synthesises a fresh value that merely belongs to the same
    /// class, chosen without reference to whatever source value originally
    /// triggered detection. [`AnomalyKind::corrupt_avoiding`] is the variant
    /// that also guarantees the result does not coincide with that source.
    ///
    /// Each arm is built to break the corresponding [`detect`] check
    /// regardless of the input's content or of where in the value the
    /// corruption lands — never relying on a lucky draw from `rng` to get
    /// there, since the detect/corrupt round trip has to hold every time.
    ///
    /// `norm` is the column's observed length profile. Only
    /// [`AnomalyKind::TruncatedText`] needs it, to cut relative to the
    /// column's own scale rather than to the value's own length; every
    /// other arm ignores it.
    ///
    /// [`AnomalyKind::MalformedDocument`] on an integer keeps the integer's
    /// width and replaces its final digit with one that makes it a valid
    /// document of neither type, so it stays an integer and reads as
    /// malformed whichever document type its column holds.
    pub fn corrupt(self, value: Value, norm: Option<LengthStats>, rng: &mut ChaCha8Rng) -> Value {
        match self {
            AnomalyKind::MalformedEmail => with_text(value, |text| insert_at(text, ' ', rng)),
            AnomalyKind::MalformedPhone => with_text(value, |text| insert_at(text, 'x', rng)),
            AnomalyKind::MalformedDocument => match value {
                Value::Int(n) => corrupt_integer_document(n, rng),
                other => with_text(other, |text| insert_at(text, 'x', rng)),
            },
            AnomalyKind::MalformedName => with_text(value, |text| corrupt_name(text, rng)),
            AnomalyKind::OutOfRange => corrupt_out_of_range(value),
            AnomalyKind::ImpossibleDate => with_text(value, corrupt_date),
            AnomalyKind::WhitespacePadding => {
                with_text(value, |text| corrupt_whitespace(text, rng))
            }
            AnomalyKind::EncodingArtifact => with_text(value, |text| corrupt_encoding(text, rng)),
            AnomalyKind::TruncatedText => with_text(value, |text| corrupt_truncated(text, norm)),
            AnomalyKind::UnexpectedNull => corrupt_unexpected_null(rng),
        }
    }

    /// [`AnomalyKind::corrupt`], except that the result does not satisfy
    /// `reproduces_source` whenever this kind can produce such a value.
    ///
    /// The draw is exactly [`AnomalyKind::corrupt`]'s, from the same `rng`
    /// state, and is returned unchanged whenever it does not reproduce the
    /// source. Otherwise the first of this kind's fixed alternatives that
    /// does not reproduce the source is returned instead, chosen without
    /// drawing from `rng`, so every later draw from the stream lands where
    /// it would have anyway. When no alternative qualifies the draw is
    /// returned as is. The source itself is never an input here: it is
    /// consulted only through `reproduces_source`, to be excluded.
    pub fn corrupt_avoiding(
        self,
        value: Value,
        norm: Option<LengthStats>,
        rng: &mut ChaCha8Rng,
        reproduces_source: impl Fn(&Value) -> bool,
    ) -> Value {
        let drawn = self.corrupt(value.clone(), norm, rng);
        if !reproduces_source(&drawn) {
            return drawn;
        }
        self.alternatives(&value)
            .into_iter()
            .find(|candidate| !reproduces_source(candidate))
            .unwrap_or(drawn)
    }

    /// Every value of this kind [`AnomalyKind::corrupt_avoiding`] may fall
    /// back on for `value`, in a fixed order. Each is an output
    /// [`AnomalyKind::corrupt`] can itself produce for `value`, apart from
    /// [`EMPTY_NULL_SENTINEL`]. Kinds whose corruption is a deterministic
    /// function of `value` have none: there is nothing else to fall back on.
    fn alternatives(self, value: &Value) -> Vec<Value> {
        let Value::Text(text) = value else {
            return match self {
                AnomalyKind::UnexpectedNull => null_sentinel_alternatives(),
                AnomalyKind::MalformedDocument => match value {
                    Value::Int(n) => integer_document_corruptions(*n)
                        .into_iter()
                        .map(Value::Int)
                        .collect(),
                    Value::Text(_) | Value::Float(_) | Value::Bool(_) | Value::Null => Vec::new(),
                },
                AnomalyKind::MalformedEmail
                | AnomalyKind::MalformedPhone
                | AnomalyKind::MalformedName
                | AnomalyKind::OutOfRange
                | AnomalyKind::ImpossibleDate
                | AnomalyKind::WhitespacePadding
                | AnomalyKind::EncodingArtifact
                | AnomalyKind::TruncatedText => Vec::new(),
            };
        };
        let texts: Vec<String> = match self {
            AnomalyKind::MalformedEmail => every_insertion(text, ' '),
            AnomalyKind::MalformedPhone | AnomalyKind::MalformedDocument => {
                every_insertion(text, 'x')
            }
            AnomalyKind::MalformedName => name_corruptions(text),
            AnomalyKind::WhitespacePadding => whitespace_corruptions(text),
            AnomalyKind::EncodingArtifact => encoding_corruptions(text),
            AnomalyKind::UnexpectedNull => return null_sentinel_alternatives(),
            AnomalyKind::OutOfRange | AnomalyKind::ImpossibleDate | AnomalyKind::TruncatedText => {
                Vec::new()
            }
        };
        texts.into_iter().map(Value::Text).collect()
    }
}

/// The shapes that make a person's name read as [`AnomalyKind::MalformedName`]:
/// what a dump that came from a form with no server-side validation
/// recurrently holds. A name carries an embedded digit alongside at most one
/// of the two casings, so `ANA1` is both [`NameShape::AllUppercase`] and
/// [`NameShape::EmbeddedDigit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameShape {
    /// Holds an ASCII letter, and every ASCII letter it holds is uppercase.
    AllUppercase,
    /// Holds an ASCII letter, and every ASCII letter it holds is lowercase.
    AllLowercase,
    /// Holds an ASCII digit anywhere.
    EmbeddedDigit,
}

/// Positions, per kind, of the anomalies found in one column. Positions are
/// row indices, never values: an index cannot be production PII, which is
/// what lets this type stay unexported from the report while still being
/// freely computable and freely tested.
///
/// `name_shapes` holds, for every position of [`AnomalyKind::MalformedName`]
/// and for no other, the [`NameShape`]s its name was found in. Only
/// [`detect`] fills it; a scan built without it re-injects
/// [`AnomalyKind::MalformedName`] through [`AnomalyKind::corrupt_avoiding`].
/// A shape is a class, never a value, so it carries no more of the source
/// than a position does.
#[derive(Debug, Clone, Default)]
pub struct AnomalyScan {
    pub positions: BTreeMap<AnomalyKind, Vec<usize>>,
    pub name_shapes: BTreeMap<usize, BTreeSet<NameShape>>,
}

impl AnomalyScan {
    pub fn counts(&self) -> BTreeMap<AnomalyKind, usize> {
        self.positions.iter().map(|(k, v)| (*k, v.len())).collect()
    }

    /// How many malformed names carry each [`NameShape`]. A name in two
    /// shapes counts under both, so the counts can sum to more than the
    /// [`AnomalyKind::MalformedName`] count.
    pub fn name_shape_counts(&self) -> BTreeMap<NameShape, usize> {
        let mut counts = BTreeMap::new();
        for &shape in self.name_shapes.values().flatten() {
            *counts.entry(shape).or_insert(0) += 1;
        }
        counts
    }
}

/// Scans `column` for every [`AnomalyKind`] relevant to `profile`'s semantic
/// type, reusing `profile`'s own length and distribution statistics rather
/// than recomputing them. A length-suppressed column (every direct
/// identifier, per [`crate::profile::Distribution::new`]) is therefore never
/// checked for [`AnomalyKind::TruncatedText`] either — the same privacy
/// boundary that hides a direct identifier's length from the report hides it
/// from this detector too.
pub fn detect(column: &ColumnData, profile: &ColumnProfile) -> AnomalyScan {
    let mut positions: BTreeMap<AnomalyKind, Vec<usize>> = BTreeMap::new();
    let mut name_shapes: BTreeMap<usize, BTreeSet<NameShape>> = BTreeMap::new();

    for (row, value) in column.values.iter().enumerate() {
        match value {
            Value::Text(text) => {
                detect_text(profile, text, row, &mut positions, &mut name_shapes);
            }
            Value::Int(n) => {
                detect_integer_document(profile, *n, row, &mut positions);
                detect_numeric(profile, *n as f64, row, &mut positions);
            }
            Value::Float(f) => detect_numeric(profile, *f, row, &mut positions),
            Value::Null | Value::Bool(_) => {}
        }
    }

    AnomalyScan {
        positions,
        name_shapes,
    }
}

/// Converts a scan's per-kind counts into per-kind rates over `total` rows,
/// the same denominator [`crate::profile::ColumnProfile::null_rate`] uses.
pub fn rates(scan: &AnomalyScan, total: usize) -> BTreeMap<AnomalyKind, f64> {
    scan.counts()
        .into_iter()
        .map(|(kind, count)| (kind, share(count, total)))
        .collect()
}

fn share(count: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

/// Re-injects each kind found in `scan` at exactly the row positions where
/// it was originally observed, rather than at freshly chosen ones: a source
/// row holding both a malformed email and a malformed CPF is one bad
/// data-entry event, and drawing independent positions for each kind would
/// turn it into two broken rows that never existed in the source. `values`
/// is assumed already anonymized — [`AnomalyKind::corrupt`] manufactures a
/// fresh defect of the same class at that position, never replaying the
/// value that was there when `scan` was taken.
///
/// Kinds are applied in [`AnomalyKind`]'s `Ord` sequence (the order
/// `scan.positions` iterates in), so a row flagged under more than one kind
/// is corrupted by every one of them, in that fixed order — overlap is
/// exactly what a single row with several defects looks like, and is
/// preserved here rather than resolved away.
///
/// A position of [`AnomalyKind::MalformedName`] with an entry in
/// `scan.name_shapes` is written in exactly those [`NameShape`]s, without
/// drawing from `rng`, so a column keeps its mix of malformed names row for
/// row. Only the shape is taken from the source; every letter comes from the
/// anonymized value.
///
/// `reproduces_source(row, candidate)` says whether writing `candidate` at
/// `row` would reproduce that row's source. Every other position goes
/// through [`AnomalyKind::corrupt_avoiding`]. Either way, whenever the kind
/// can produce a value that does not reproduce the source, the value written
/// is one: a defect re-injected onto an anonymized value does not turn it
/// back into the source cell it replaced. The source is consulted only to be
/// excluded, never replayed.
///
/// Returns the count of positions actually corrupted per kind, which is
/// `positions.len()` unless `scan` was taken over a different row count
/// than `values` (no caller in this crate does that). The caller is
/// expected to recount anomalies on the result rather than trust this
/// return value as a guarantee: corrupting a row under one kind can
/// incidentally make an earlier kind's corruption on the same row
/// undetectable, and only a fresh [`detect`] pass catches that honestly.
pub fn reinject(
    values: &mut [Value],
    scan: &AnomalyScan,
    norm: Option<LengthStats>,
    rng: &mut ChaCha8Rng,
    reproduces_source: impl Fn(usize, &Value) -> bool,
) -> BTreeMap<AnomalyKind, usize> {
    let n = values.len();
    let mut achieved = BTreeMap::new();

    for (&kind, positions) in &scan.positions {
        let mut count = 0;
        for &position in positions {
            if position < n {
                let value = values[position].clone();
                let avoiding_source = |candidate: &Value| reproduces_source(position, candidate);
                values[position] = match (kind, scan.name_shapes.get(&position)) {
                    (AnomalyKind::MalformedName, Some(shapes)) => {
                        corrupt_name_in_shapes_avoiding(value, shapes, avoiding_source)
                    }
                    _ => kind.corrupt_avoiding(value, norm, rng, avoiding_source),
                };
                count += 1;
            }
        }
        achieved.insert(kind, count);
    }

    achieved
}

fn record(positions: &mut BTreeMap<AnomalyKind, Vec<usize>>, kind: AnomalyKind, row: usize) {
    positions.entry(kind).or_default().push(row);
}

/// Scans one text value for every anomaly kind relevant to `profile`.
///
/// Shape is judged against `text` trimmed, not raw, so a padded but
/// otherwise well-formed email, phone, document, name or date is
/// classified by its primary defect — padding — rather than also being
/// reported as malformed on account of the padding alone.
///
/// A null-sentinel match returns early: whatever corruption produced
/// `text`, a value that trims to empty or lands on a literal entry in
/// [`NULL_SENTINELS`] reads as a missing value before anything else, the
/// same judgement call this detector already makes for genuine production
/// nulls written as `"n/a"` or `"--"`. A corruption that happens to
/// collapse into that shape is reported as [`AnomalyKind::UnexpectedNull`]
/// rather than its own kind — an accepted, narrow edge case rather than one
/// a real column is likely to hit at any scale.
fn detect_text(
    profile: &ColumnProfile,
    text: &str,
    row: usize,
    positions: &mut BTreeMap<AnomalyKind, Vec<usize>>,
    name_shapes_by_row: &mut BTreeMap<usize, BTreeSet<NameShape>>,
) {
    if is_null_sentinel(text) {
        record(positions, AnomalyKind::UnexpectedNull, row);
        return;
    }

    if has_whitespace_anomaly(text) {
        record(positions, AnomalyKind::WhitespacePadding, row);
    }
    if has_encoding_artifact(text) {
        record(positions, AnomalyKind::EncodingArtifact, row);
    }
    if is_truncated(text, profile.length) {
        record(positions, AnomalyKind::TruncatedText, row);
    }

    let trimmed = text.trim();
    match profile.semantic_type {
        SemanticType::Email if !SemanticType::Email.matches_text(trimmed) => {
            record(positions, AnomalyKind::MalformedEmail, row);
        }
        SemanticType::Cpf | SemanticType::Cnpj
            if !document_is_valid(profile.semantic_type, trimmed) =>
        {
            record(positions, AnomalyKind::MalformedDocument, row);
        }
        SemanticType::Phone if !SemanticType::Phone.matches_text(trimmed) => {
            record(positions, AnomalyKind::MalformedPhone, row);
        }
        SemanticType::FullName => {
            let shapes = name_shapes(trimmed);
            if !shapes.is_empty() {
                record(positions, AnomalyKind::MalformedName, row);
                name_shapes_by_row.insert(row, shapes);
            }
        }
        SemanticType::Date if is_impossible_date(trimmed) => {
            record(positions, AnomalyKind::ImpossibleDate, row);
        }
        _ => {}
    }
}

/// Flags `n` as [`AnomalyKind::MalformedDocument`] when its column is
/// classified [`SemanticType::Cpf`] or [`SemanticType::Cnpj`] and `n` is not
/// a valid document of that type, as [`classify::is_valid_document`] judges
/// one: the column's type, not the value's digit count, decides which
/// document `n` must be.
fn detect_integer_document(
    profile: &ColumnProfile,
    n: i64,
    row: usize,
    positions: &mut BTreeMap<AnomalyKind, Vec<usize>>,
) {
    if matches!(
        profile.semantic_type,
        SemanticType::Cpf | SemanticType::Cnpj
    ) && !classify::is_valid_document(profile.semantic_type, &Value::Int(n))
    {
        record(positions, AnomalyKind::MalformedDocument, row);
    }
}

fn detect_numeric(
    profile: &ColumnProfile,
    raw: f64,
    row: usize,
    positions: &mut BTreeMap<AnomalyKind, Vec<usize>>,
) {
    if profile.semantic_type != SemanticType::NumericId {
        return;
    }
    if let Distribution::Numeric(numeric) = &profile.distribution {
        if is_out_of_range(raw, numeric) {
            record(positions, AnomalyKind::OutOfRange, row);
        }
    }
}

fn document_is_valid(semantic_type: SemanticType, text: &str) -> bool {
    match semantic_type {
        SemanticType::Cpf | SemanticType::Cnpj => {
            classify::is_valid_text_document(semantic_type, text)
        }
        _ => true,
    }
}

fn is_null_sentinel(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || NULL_SENTINELS.contains(&trimmed.to_lowercase().as_str())
}

/// Leading/trailing padding, a run of two or more interior spaces, or a tab,
/// newline or carriage return anywhere in what is meant to be a single-line
/// field.
fn has_whitespace_anomaly(text: &str) -> bool {
    if text.trim() != text {
        return true;
    }
    if text.contains(['\t', '\n', '\r']) {
        return true;
    }
    let mut previous_was_space = false;
    for c in text.chars() {
        if c == ' ' {
            if previous_was_space {
                return true;
            }
            previous_was_space = true;
        } else {
            previous_was_space = false;
        }
    }
    false
}

/// The replacement character is what a lone UTF-16 surrogate becomes by the
/// time a JSON decoder has produced a valid Rust `String` from it, so
/// checking for it covers both that case and an explicit mojibake
/// replacement glyph. [`MOJIBAKE_MARKERS`] covers the other common shape:
/// valid UTF-8 re-decoded as Latin-1.
fn has_encoding_artifact(text: &str) -> bool {
    text.contains('\u{FFFD}') || MOJIBAKE_MARKERS.iter().any(|marker| text.contains(marker))
}

fn is_truncated(text: &str, norm: Option<LengthStats>) -> bool {
    let Some(norm) = norm else { return false };
    if norm.mean <= 0.0 {
        return false;
    }
    let len = text.chars().count();
    len > 0 && (len as f64) < norm.mean * TRUNCATION_RATIO
}

/// Mean and standard deviation have a breakdown point of `1/n`: a handful of
/// outliers among many rows poisons the very statistic meant to find them,
/// pulling the mean toward the outliers and inflating the standard
/// deviation computed around it until the outliers no longer look far from
/// it. Median and median absolute deviation have a 50% breakdown point, so
/// this stays accurate at any realistic injection rate.
fn is_out_of_range(value: f64, numeric: &NumericProfile) -> bool {
    if numeric.mad <= f64::EPSILON {
        return false;
    }
    let robust_sigma = numeric.mad * MAD_TO_SIGMA;
    ((value - numeric.median) / robust_sigma).abs() > OUT_OF_RANGE_SIGMA
}

/// Every [`NameShape`] `text` is in; empty for a well-formed name.
fn name_shapes(text: &str) -> BTreeSet<NameShape> {
    let mut shapes = BTreeSet::new();
    if text.chars().any(|c| c.is_ascii_digit()) {
        shapes.insert(NameShape::EmbeddedDigit);
    }
    let letters: Vec<char> = text.chars().filter(char::is_ascii_alphabetic).collect();
    if !letters.is_empty() {
        if letters.iter().all(char::is_ascii_uppercase) {
            shapes.insert(NameShape::AllUppercase);
        } else if letters.iter().all(char::is_ascii_lowercase) {
            shapes.insert(NameShape::AllLowercase);
        }
    }
    shapes
}

fn is_impossible_date(text: &str) -> bool {
    match parse_date_fields(text) {
        Some((year, month, day)) => {
            year == 0 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month)
        }
        None => true,
    }
}

/// Parses the two shapes [`classify::SemanticType::Date`] accepts, without
/// validating that the fields it pulls out describe a real day — that
/// judgement belongs to [`is_impossible_date`], which is also what makes
/// `0000-00-00` parse successfully here and get rejected there, matching
/// MySQL's own zero date verbatim rather than failing to parse at all.
pub(crate) fn parse_date_fields(text: &str) -> Option<(u32, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 {
        return None;
    }
    if bytes[4] == b'-' && bytes[7] == b'-' {
        return Some((
            text[0..4].parse().ok()?,
            text[5..7].parse().ok()?,
            text[8..10].parse().ok()?,
        ));
    }
    let separator = bytes[2];
    if (separator == b'-' || separator == b'/') && bytes[5] == separator {
        return Some((
            text[6..10].parse().ok()?,
            text[3..5].parse().ok()?,
            text[0..2].parse().ok()?,
        ));
    }
    None
}

pub(crate) fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap_year(year) => 29,
        _ => 28,
    }
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn with_text(value: Value, f: impl FnOnce(&str) -> String) -> Value {
    match value {
        Value::Text(text) => Value::Text(f(&text)),
        other => other,
    }
}

/// Inserts `ch` at an interior position, never the first or last, so the
/// result is never padded with a leading or trailing instance of it —
/// `insert_at` exists to break a structured value's shape, not to
/// accidentally manufacture whitespace padding at a boundary. Degenerate
/// for inputs shorter than two characters, where no interior position
/// exists: `ch` is appended there, since nothing more specific is possible.
fn insert_at(text: &str, ch: char, rng: &mut ChaCha8Rng) -> String {
    let positions = insertion_positions(text);
    let position = if positions.is_empty() {
        positions.start
    } else {
        rng.random_range(positions)
    };
    inserted(text, ch, position)
}

/// The positions [`insert_at`] chooses among: every interior one, or the
/// end alone (an empty range starting there) when `text` has fewer than two
/// characters.
fn insertion_positions(text: &str) -> std::ops::Range<usize> {
    let len = text.chars().count();
    if len >= 2 { 1..len } else { len..len }
}

fn inserted(text: &str, ch: char, position: usize) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    chars.insert(position, ch);
    chars.into_iter().collect()
}

/// Every value [`insert_at`] can produce for `text`, in position order.
fn every_insertion(text: &str, ch: char) -> Vec<String> {
    let positions = insertion_positions(text);
    if positions.is_empty() {
        return vec![inserted(text, ch, positions.start)];
    }
    positions
        .map(|position| inserted(text, ch, position))
        .collect()
}

/// The casing arms are only a corruption when `text` has a cased ASCII
/// letter for [`name_shapes`] to notice; a value with none
/// (`"中文"`) passes through both unchanged, so they are only drawn when one
/// exists. Otherwise the digit-append arm runs unconditionally, which always
/// changes the value and always introduces a digit that check flags.
fn corrupt_name(text: &str, rng: &mut ChaCha8Rng) -> String {
    if !text.chars().any(|c| c.is_ascii_alphabetic()) {
        return name_with_digit(text);
    }
    let choice: u8 = rng.random_range(0..3);
    match choice {
        0 => text.to_uppercase(),
        1 => text.to_lowercase(),
        _ => name_with_digit(text),
    }
}

fn name_with_digit(text: &str) -> String {
    format!("{text}1")
}

/// `text` written in `shapes`: cased as the set says, then with `digit`
/// appended when it holds [`NameShape::EmbeddedDigit`]. A `text` with no
/// ASCII letter cannot express a casing, so it gets `digit` appended
/// whatever `shapes` holds, as [`corrupt_name`] does for it: still a
/// malformed name, though not in the casing asked for.
fn name_in_shapes(text: &str, shapes: &BTreeSet<NameShape>, digit: char) -> String {
    if !text.chars().any(|c| c.is_ascii_alphabetic()) {
        return format!("{text}{digit}");
    }
    let cased = if shapes.contains(&NameShape::AllUppercase) {
        text.to_uppercase()
    } else if shapes.contains(&NameShape::AllLowercase) {
        text.to_lowercase()
    } else {
        text.to_owned()
    };
    if shapes.contains(&NameShape::EmbeddedDigit) {
        format!("{cased}{digit}")
    } else {
        cased
    }
}

/// Every name [`corrupt_name_in_shapes_avoiding`] may write for `text`, in
/// the order it tries them: `text` in `shapes` with the digit `1`; when
/// `shapes` holds [`NameShape::EmbeddedDigit`], the same with each other
/// digit, `2` to `9` then `0`; then [`name_corruptions`], which keep the kind
/// but not necessarily the shapes.
fn shaped_name_alternatives(text: &str, shapes: &BTreeSet<NameShape>) -> Vec<String> {
    let digits: Vec<char> = if shapes.contains(&NameShape::EmbeddedDigit) {
        ('1'..='9').chain(std::iter::once('0')).collect()
    } else {
        vec!['1']
    };
    digits
        .into_iter()
        .map(|digit| name_in_shapes(text, shapes, digit))
        .chain(name_corruptions(text))
        .collect()
}

/// Writes the anonymized `value` as a malformed name in `shapes`, without
/// drawing from any stream: the first of [`shaped_name_alternatives`] that
/// does not satisfy `reproduces_source`, or the first of them when every one
/// does. A row whose shaped name would reproduce its source therefore keeps
/// the kind, and its shapes too whenever another digit tells it apart. A
/// non-text value passes through unchanged.
fn corrupt_name_in_shapes_avoiding(
    value: Value,
    shapes: &BTreeSet<NameShape>,
    reproduces_source: impl Fn(&Value) -> bool,
) -> Value {
    let Value::Text(text) = value else {
        return value;
    };
    let candidates: Vec<Value> = shaped_name_alternatives(&text, shapes)
        .into_iter()
        .map(Value::Text)
        .collect();
    candidates
        .iter()
        .find(|candidate| !reproduces_source(candidate))
        .or(candidates.first())
        .cloned()
        .unwrap_or(Value::Text(text))
}

/// Every value [`corrupt_name`] can produce for `text`, in the order it
/// draws among them.
fn name_corruptions(text: &str) -> Vec<String> {
    if text.chars().any(|c| c.is_ascii_alphabetic()) {
        vec![
            text.to_uppercase(),
            text.to_lowercase(),
            name_with_digit(text),
        ]
    } else {
        vec![name_with_digit(text)]
    }
}

/// Every value [`corrupt_integer_document`] can produce for `n`, in draw
/// order: `n` with the final decimal digit of its magnitude replaced by each
/// other digit, ascending, keeping its sign and width and only the results
/// valid as neither a CPF nor a CNPJ.
fn integer_document_corruptions(n: i64) -> Vec<i64> {
    let last = (n % 10).abs();
    (0..10i64)
        .filter(|&digit| digit != last)
        .filter_map(|digit| {
            if n < 0 {
                n.checked_sub(digit - last)
            } else {
                n.checked_add(digit - last)
            }
        })
        .filter(|&candidate| {
            !classify::is_valid_integer_document(SemanticType::Cpf, candidate)
                && !classify::is_valid_integer_document(SemanticType::Cnpj, candidate)
        })
        .collect()
}

/// Draws one of [`integer_document_corruptions`] for `n`, or returns `n`
/// unchanged, without drawing, when there is none.
fn corrupt_integer_document(n: i64, rng: &mut ChaCha8Rng) -> Value {
    let candidates = integer_document_corruptions(n);
    if candidates.is_empty() {
        return Value::Int(n);
    }
    let pick = rng.random_range(0..candidates.len());
    candidates
        .get(pick)
        .copied()
        .map_or(Value::Int(n), Value::Int)
}

fn corrupt_out_of_range(value: Value) -> Value {
    match value {
        Value::Int(n) => Value::Int(n.saturating_add(1_000_000_000)),
        Value::Float(f) => Value::Float(f + 1.0e12),
        other => other,
    }
}

/// Forces the day field to `00`, so the result is always caught by
/// [`is_impossible_date`]'s `day == 0` check regardless of the rest of the
/// value — matching MySQL's own `0000-00-00` as the degenerate case when the
/// value does not even parse.
fn corrupt_date(text: &str) -> String {
    match parse_date_fields(text) {
        Some(_)
            if text.as_bytes().get(4) == Some(&b'-') && text.as_bytes().get(7) == Some(&b'-') =>
        {
            format!("{}-{}-00", &text[0..4], &text[5..7])
        }
        Some(_) => format!("00{}", &text[2..]),
        None => "0000-00-00".to_owned(),
    }
}

fn corrupt_whitespace(text: &str, rng: &mut ChaCha8Rng) -> String {
    let choice: u8 = rng.random_range(0..3);
    let [leading, trailing, tab] = whitespace_corruptions_of(text);
    match choice {
        0 => leading,
        1 => trailing,
        _ => tab,
    }
}

fn whitespace_corruptions_of(text: &str) -> [String; 3] {
    [
        format!("  {text}"),
        format!("{text}  "),
        format!("{text}\t"),
    ]
}

/// Every value [`corrupt_whitespace`] can produce for `text`.
fn whitespace_corruptions(text: &str) -> Vec<String> {
    whitespace_corruptions_of(text).into()
}

fn corrupt_encoding(text: &str, rng: &mut ChaCha8Rng) -> String {
    let [replacement, mojibake] = encoding_corruptions_of(text);
    if rng.random_bool(0.5) {
        replacement
    } else {
        mojibake
    }
}

fn encoding_corruptions_of(text: &str) -> [String; 2] {
    [
        format!("{text}\u{FFFD}"),
        format!("{text}{}", MOJIBAKE_MARKERS[0]),
    ]
}

/// Every value [`corrupt_encoding`] can produce for `text`.
fn encoding_corruptions(text: &str) -> Vec<String> {
    encoding_corruptions_of(text).into()
}

/// Keeps enough of `text` to fall below [`TRUNCATION_RATIO`] of the
/// column's observed mean length (`norm`), rather than a fraction of
/// `text`'s own length — a value already longer than the column norm would
/// otherwise survive a self-relative cut without ever reading as truncated.
/// Cuts to *half* the threshold rather than just under it, since on a small
/// column corrupting one value measurably drags the mean `detect`
/// recomputes afterwards down with it; landing just under the
/// pre-corruption threshold can land back above that lowered one, while
/// half leaves margin against it. Left unmodified, and therefore
/// undetectable, when there is no norm to fall below, or when `text` is
/// already so short (three characters or fewer) that cutting it further
/// would not leave anything resembling a truncated value rather than an
/// empty one.
fn corrupt_truncated(text: &str, norm: Option<LengthStats>) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 3 {
        return text.to_owned();
    }
    let Some(norm) = norm else {
        return text.to_owned();
    };
    let threshold = norm.mean * TRUNCATION_RATIO;
    if threshold <= 1.0 {
        return text.to_owned();
    }
    let keep = ((threshold / 2.0).floor() as usize).clamp(1, chars.len() - 1);
    chars.into_iter().take(keep).collect()
}

fn corrupt_unexpected_null(rng: &mut ChaCha8Rng) -> Value {
    let choice: usize = rng.random_range(0..NULL_SENTINELS.len());
    Value::Text(NULL_SENTINELS[choice].to_owned())
}

/// Every entry of [`NULL_SENTINELS`] in order, then [`EMPTY_NULL_SENTINEL`].
fn null_sentinel_alternatives() -> Vec<Value> {
    NULL_SENTINELS
        .iter()
        .chain(std::iter::once(&EMPTY_NULL_SENTINEL))
        .map(|sentinel| Value::Text((*sentinel).to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::classify::Classification;
    use crate::dataset::{ColumnSpec, ColumnType};
    use crate::profile;

    fn column(name: &str, ty: ColumnType, values: Vec<Value>) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty,
                nullable: true,
                pk: false,
                fk: None,
            },
            values,
        }
    }

    fn text_column(name: &str, values: &[&str]) -> ColumnData {
        column(
            name,
            ColumnType::Text,
            values.iter().map(|v| Value::Text(v.to_string())).collect(),
        )
    }

    fn profile_for(column: &ColumnData, semantic_type: SemanticType) -> ColumnProfile {
        profile::profile_column(
            &column.spec.name,
            column,
            Classification {
                semantic_type,
                confidence: 1.0,
            },
        )
    }

    fn scan_of(column: &ColumnData, semantic_type: SemanticType) -> AnomalyScan {
        let profile = profile_for(column, semantic_type);
        detect(column, &profile)
    }

    #[test]
    fn a_malformed_email_is_detected() {
        let column = text_column("email", &["ana@gmail.com", "not-an-email"]);
        let scan = scan_of(&column, SemanticType::Email);
        assert_eq!(scan.positions[&AnomalyKind::MalformedEmail], vec![1]);
    }

    #[test]
    fn a_well_formed_email_column_has_no_malformed_email_positions() {
        let column = text_column("email", &["ana@gmail.com", "bruno@outlook.com"]);
        let scan = scan_of(&column, SemanticType::Email);
        assert!(!scan.positions.contains_key(&AnomalyKind::MalformedEmail));
    }

    #[test]
    fn a_cpf_with_bad_check_digits_is_malformed_document() {
        let column = text_column("cpf", &["111.444.777-35", "111.444.777-36"]);
        let scan = scan_of(&column, SemanticType::Cpf);
        assert_eq!(scan.positions[&AnomalyKind::MalformedDocument], vec![1]);
    }

    #[test]
    fn a_phone_missing_digits_is_malformed_phone() {
        let column = text_column("phone", &["(11) 98765-4321", "11-99"]);
        let scan = scan_of(&column, SemanticType::Phone);
        assert_eq!(scan.positions[&AnomalyKind::MalformedPhone], vec![1]);
    }

    #[test]
    fn all_caps_and_digit_embedded_names_are_malformed_name() {
        let column = text_column(
            "name",
            &["Ana Souza", "BRUNO LIMA", "carla dias", "D4vi Reis"],
        );
        let scan = scan_of(&column, SemanticType::FullName);
        let flagged = scan.positions[&AnomalyKind::MalformedName].clone();
        assert_eq!(flagged, vec![1, 2, 3]);
    }

    #[test]
    fn mysql_zero_date_is_an_impossible_date() {
        let column = text_column("dob", &["2024-01-15", "0000-00-00", "2024-13-40"]);
        let scan = scan_of(&column, SemanticType::Date);
        assert_eq!(
            scan.positions[&AnomalyKind::ImpossibleDate],
            vec![1, 2],
            "both the MySQL zero date and an out-of-range month/day must be flagged"
        );
    }

    #[test]
    fn a_valid_date_is_not_impossible() {
        let column = text_column("dob", &["2024-02-29", "31/12/2023"]);
        let scan = scan_of(&column, SemanticType::Date);
        assert!(!scan.positions.contains_key(&AnomalyKind::ImpossibleDate));
    }

    #[test]
    fn february_29_on_a_non_leap_year_is_impossible() {
        let column = text_column("dob", &["2023-02-29"]);
        let scan = scan_of(&column, SemanticType::Date);
        assert_eq!(scan.positions[&AnomalyKind::ImpossibleDate], vec![0]);
    }

    #[test]
    fn padded_and_tab_ridden_text_is_whitespace_padding() {
        let column = text_column(
            "city",
            &[
                "Sao Paulo",
                "  Rio de Janeiro",
                "Recife\t",
                "Belo  Horizonte",
            ],
        );
        let scan = scan_of(&column, SemanticType::FreeText);
        assert_eq!(
            scan.positions[&AnomalyKind::WhitespacePadding],
            vec![1, 2, 3]
        );
    }

    #[test]
    fn mojibake_and_replacement_characters_are_encoding_artifacts() {
        let column = text_column("city", &["São Paulo", "SÃ£o Paulo", "S\u{FFFD}o Paulo"]);
        let scan = scan_of(&column, SemanticType::FreeText);
        assert_eq!(scan.positions[&AnomalyKind::EncodingArtifact], vec![1, 2]);
    }

    #[test]
    fn null_sentinels_are_unexpected_null_and_nothing_else() {
        let column = text_column("notes", &["a real note", "N/A", "", "null", "   "]);
        let scan = scan_of(&column, SemanticType::FreeText);
        assert_eq!(
            scan.positions[&AnomalyKind::UnexpectedNull],
            vec![1, 2, 3, 4]
        );
        assert!(!scan.positions.contains_key(&AnomalyKind::WhitespacePadding));
    }

    #[test]
    fn a_value_far_shorter_than_the_column_norm_is_truncated() {
        let column = text_column(
            "bio",
            &[
                "a reasonably long piece of free text",
                "another reasonably long sentence here",
                "yet another normal length sentence",
                "x",
            ],
        );
        let scan = scan_of(&column, SemanticType::FreeText);
        assert_eq!(scan.positions[&AnomalyKind::TruncatedText], vec![3]);
    }

    #[test]
    fn length_is_not_checked_for_a_direct_identifier() {
        let column = text_column("full_name", &["Ana Paula Souza De Oliveira Lima", "Jo"]);
        let scan = scan_of(&column, SemanticType::FullName);
        assert!(!scan.positions.contains_key(&AnomalyKind::TruncatedText));
    }

    #[test]
    fn a_numeric_outlier_many_sigma_away_is_out_of_range() {
        let mut values: Vec<Value> = (20..60).map(Value::Int).collect();
        values.push(Value::Int(1_000_000));
        let column = column("age", ColumnType::Integer, values);
        let scan = scan_of(&column, SemanticType::NumericId);
        assert_eq!(scan.positions[&AnomalyKind::OutOfRange], vec![40]);
    }

    #[test]
    fn a_constant_numeric_column_never_flags_out_of_range() {
        let values: Vec<Value> = std::iter::repeat_n(Value::Int(5), 20).collect();
        let column = column("flag", ColumnType::Integer, values);
        let scan = scan_of(&column, SemanticType::NumericId);
        assert!(!scan.positions.contains_key(&AnomalyKind::OutOfRange));
    }

    /// Exhaustiveness guard: a match with no wildcard arm, so adding a
    /// variant to [`AnomalyKind`] without covering it here fails to compile.
    fn assert_every_variant_is_covered(kind: AnomalyKind) {
        match kind {
            AnomalyKind::MalformedEmail
            | AnomalyKind::MalformedDocument
            | AnomalyKind::MalformedPhone
            | AnomalyKind::MalformedName
            | AnomalyKind::OutOfRange
            | AnomalyKind::ImpossibleDate
            | AnomalyKind::WhitespacePadding
            | AnomalyKind::EncodingArtifact
            | AnomalyKind::TruncatedText
            | AnomalyKind::UnexpectedNull => {}
        }
    }

    #[test]
    fn all_lists_every_anomaly_kind_variant_exactly_once() {
        let seen: BTreeSet<AnomalyKind> = AnomalyKind::ALL.iter().copied().collect();
        for kind in AnomalyKind::ALL {
            assert_every_variant_is_covered(*kind);
        }
        assert_eq!(seen.len(), AnomalyKind::ALL.len(), "ALL names a duplicate");
    }

    /// For every kind, corrupting a clean value must produce a value that
    /// `detect` reports back as that same kind, that kind *alone*, and never
    /// the input left unchanged. Checked over several realistic values and
    /// many seeds per kind, rather than one cooperative fixture value and a
    /// single seed, since that is exactly what let a cross-kind
    /// contamination bug and a silent no-op both ship undetected.
    ///
    /// The corrupted value sits alongside clean neighbours rather than
    /// alone, because two kinds (`OutOfRange`, `TruncatedText`) are judged
    /// relative to the column's own observed norm: a column of one value has
    /// no norm to deviate from.
    #[test]
    fn every_anomaly_kind_round_trips_as_exactly_itself_across_seeds_and_values() {
        const SEEDS: u64 = 25;

        for &kind in AnomalyKind::ALL {
            let (semantic_type, baseline) = baseline_for(kind);
            let norm = norm_for(semantic_type, &baseline);

            for candidate in candidates_for(kind) {
                for seed in 0..SEEDS {
                    let mut values = baseline.clone();
                    let mut rng = crate::rng::derive(seed, b"round-trip");
                    let corrupted = kind.corrupt(candidate.clone(), norm, &mut rng);
                    assert_ne!(
                        corrupted, candidate,
                        "seed {seed}: corrupt({kind:?}) on {candidate:?} must not return its \
                         input unchanged"
                    );
                    values[0] = corrupted;

                    let probe = column("value", column_type_for(&values[0]), values);
                    let profile = profile_for(&probe, semantic_type);
                    let scan = detect(&probe, &profile);

                    let hits: Vec<AnomalyKind> = scan
                        .positions
                        .iter()
                        .filter(|(_, positions)| positions.contains(&0))
                        .map(|(&k, _)| k)
                        .collect();
                    assert_eq!(
                        hits,
                        vec![kind],
                        "seed {seed}: corrupt({kind:?}) on {candidate:?} must detect as \
                         {kind:?} alone, got {hits:?}"
                    );
                }
            }
        }
    }

    /// Regression: padding an otherwise valid email must register as
    /// padding, not as a malformed address — the address is well-formed
    /// once the padding is trimmed away, so padding is its whole defect.
    #[test]
    fn whitespace_padding_on_an_email_column_does_not_also_flag_malformed_email() {
        for seed in 0..50u64 {
            let mut rng = crate::rng::derive(seed, b"whitespace-on-email");
            let mut values = baseline_for(AnomalyKind::MalformedEmail).1;
            values[0] = AnomalyKind::WhitespacePadding.corrupt(values[0].clone(), None, &mut rng);

            let probe = column("value", ColumnType::Text, values);
            let profile = profile_for(&probe, SemanticType::Email);
            let scan = detect(&probe, &profile);

            assert!(
                scan.positions
                    .get(&AnomalyKind::WhitespacePadding)
                    .is_some_and(|p| p.contains(&0)),
                "seed {seed}: the padding itself must still be detected"
            );
            assert!(
                !scan
                    .positions
                    .get(&AnomalyKind::MalformedEmail)
                    .is_some_and(|p| p.contains(&0)),
                "seed {seed}: padding alone must not also read as a malformed email"
            );
        }
    }

    /// Median and median absolute deviation have a 50% breakdown point, so
    /// they must catch every outlier at a realistic injection rate — a
    /// handful of injected outliers among `n` values would poison the mean
    /// and standard deviation, the very statistics meant to find them, so
    /// those would detect none.
    #[test]
    fn out_of_range_detects_every_outlier_at_a_realistic_injection_rate() {
        for &(n, k) in &[(50usize, 3usize), (100usize, 6usize)] {
            let mut values: Vec<Value> = (0..n as i64).map(Value::Int).collect();
            for value in values.iter_mut().take(k) {
                *value = corrupt_out_of_range(value.clone());
            }
            let probe = column("value", ColumnType::Integer, values);
            let scan = scan_of(&probe, SemanticType::NumericId);
            let detected = scan
                .positions
                .get(&AnomalyKind::OutOfRange)
                .map_or(0, Vec::len);
            assert_eq!(
                detected, k,
                "n={n} k={k}: expected all {k} injected outliers detected, got {detected}"
            );
        }
    }

    /// The RNG for one value's norm, as [`AnomalyKind::corrupt`] needs it for
    /// [`AnomalyKind::TruncatedText`]: profiled from `values` as they stand,
    /// mirroring the column norm `corrupt` would see in the pipeline, before
    /// the corruption under test is applied.
    fn norm_for(semantic_type: SemanticType, values: &[Value]) -> Option<LengthStats> {
        let probe = column("probe", column_type_for(&values[0]), values.to_vec());
        profile_for(&probe, semantic_type).length
    }

    /// Several realistic values to try at the corrupted position, so the
    /// round-trip guarantee does not rest on one cooperative fixture value.
    fn candidates_for(kind: AnomalyKind) -> Vec<Value> {
        fn texts(values: &[&str]) -> Vec<Value> {
            values
                .iter()
                .map(|v| Value::Text((*v).to_owned()))
                .collect()
        }

        match kind {
            AnomalyKind::MalformedEmail => texts(&[
                "ana@gmail.com",
                "bruno.lima@outlook.com",
                "carla_souza99@yahoo.com.br",
            ]),
            AnomalyKind::MalformedPhone => {
                texts(&["(11) 98765-4321", "(21) 91234-5678", "(31) 99876-5432"])
            }
            AnomalyKind::MalformedDocument => {
                texts(&["111.444.777-35", "123.456.789-09", "529.982.247-25"])
            }
            // "中文" has no cased ASCII letter, so it exercises
            // `corrupt_name`'s digit-append fallback.
            AnomalyKind::MalformedName => texts(&["Ana Souza", "Bruno Lima", "中文"]),
            AnomalyKind::OutOfRange => vec![Value::Int(20)],
            AnomalyKind::ImpossibleDate => texts(&["2024-01-15", "2023-06-10", "31/12/2022"]),
            AnomalyKind::WhitespacePadding | AnomalyKind::EncodingArtifact => texts(&[
                "a normal sentence",
                "another sentence here",
                "yet one more sentence",
            ]),
            AnomalyKind::TruncatedText => texts(&[
                "a normal length sentence for testing",
                "another normal length sentence here",
                "yet another normal length sentence",
            ]),
            AnomalyKind::UnexpectedNull => texts(&[
                "a normal sentence",
                "another one here",
                "a third normal sentence",
            ]),
        }
    }

    fn column_type_for(value: &Value) -> ColumnType {
        match value {
            Value::Int(_) => ColumnType::Integer,
            Value::Float(_) => ColumnType::Float,
            Value::Bool(_) => ColumnType::Boolean,
            Value::Text(_) | Value::Null => ColumnType::Text,
        }
    }

    /// A small column of values `detect` would *not* flag as `kind`, paired
    /// with the semantic type it belongs to. This is the input side of the
    /// round-trip test: `corrupt` has to be what makes position 0
    /// detectable, not a value that was already going to trigger the check.
    fn baseline_for(kind: AnomalyKind) -> (SemanticType, Vec<Value>) {
        fn texts(values: &[&str]) -> Vec<Value> {
            values
                .iter()
                .map(|v| Value::Text((*v).to_owned()))
                .collect()
        }

        match kind {
            AnomalyKind::MalformedEmail => (
                SemanticType::Email,
                texts(&["ana@gmail.com", "bruno@outlook.com", "carla@gmail.com"]),
            ),
            AnomalyKind::MalformedPhone => (
                SemanticType::Phone,
                texts(&["(11) 98765-4321", "(21) 91234-5678", "(31) 99876-5432"]),
            ),
            AnomalyKind::MalformedDocument => (
                SemanticType::Cpf,
                texts(&["111.444.777-35", "123.456.789-09", "529.982.247-25"]),
            ),
            AnomalyKind::MalformedName => (
                SemanticType::FullName,
                texts(&["Ana Souza", "Bruno Lima", "Carla Dias"]),
            ),
            // A single far outlier's z-score against `n` clustered values is
            // bounded above by `sqrt(n - 1)`, regardless of the outlier's own
            // magnitude: with too few neighbours, no corruption can clear
            // `OUT_OF_RANGE_SIGMA`. Twenty neighbours keeps the ceiling above
            // the threshold with room to spare.
            AnomalyKind::OutOfRange => {
                (SemanticType::NumericId, (20..40).map(Value::Int).collect())
            }
            AnomalyKind::ImpossibleDate => (
                SemanticType::Date,
                texts(&["2024-01-15", "2023-06-10", "2022-12-25"]),
            ),
            AnomalyKind::WhitespacePadding => (
                SemanticType::FreeText,
                texts(&[
                    "a normal sentence",
                    "another normal one",
                    "yet another sentence",
                ]),
            ),
            AnomalyKind::EncodingArtifact => (
                SemanticType::FreeText,
                texts(&[
                    "a normal sentence",
                    "another normal one",
                    "yet another sentence",
                ]),
            ),
            AnomalyKind::TruncatedText => (
                SemanticType::FreeText,
                texts(&[
                    "a normal length sentence for testing",
                    "another normal length sentence here",
                    "yet another normal length sentence",
                ]),
            ),
            AnomalyKind::UnexpectedNull => (
                SemanticType::FreeText,
                texts(&[
                    "a normal sentence",
                    "another one here",
                    "a third normal sentence",
                ]),
            ),
        }
    }

    fn scan_for(kind: AnomalyKind, positions: &[usize]) -> AnomalyScan {
        let mut scan = AnomalyScan::default();
        scan.positions.insert(kind, positions.to_vec());
        scan
    }

    #[test]
    fn reinject_achieves_the_exact_target_count_for_one_kind() {
        let (_, mut values) = baseline_for(AnomalyKind::MalformedEmail);
        values.extend((0..27).map(|i| Value::Text(format!("clean{i}@gmail.com"))));
        let scan = scan_for(AnomalyKind::MalformedEmail, &[0, 1, 2]);

        let mut rng = crate::rng::derive(5, b"reinject-one-kind");
        let achieved = reinject(&mut values, &scan, None, &mut rng, |_, _| false);

        assert_eq!(achieved[&AnomalyKind::MalformedEmail], 3);
        let probe = column("value", ColumnType::Text, values);
        let profile = profile_for(&probe, SemanticType::Email);
        let recount = detect(&probe, &profile).counts();
        assert_eq!(recount.get(&AnomalyKind::MalformedEmail), Some(&3));
    }

    /// A source row carrying both defects is one bad data-entry event, not
    /// two independently placed ones, so a row scanned under two kinds must
    /// come back out flagged under both — never resolved into disjoint sets
    /// by whichever kind claims it first.
    #[test]
    fn reinject_applies_every_kind_flagged_on_an_overlapping_row() {
        let mut values: Vec<Value> = (0..40)
            .map(|i| Value::Text(format!("clean sentence number {i}")))
            .collect();
        let mut scan = AnomalyScan::default();
        scan.positions
            .insert(AnomalyKind::WhitespacePadding, (0..8).collect());
        scan.positions
            .insert(AnomalyKind::EncodingArtifact, (0..8).collect());

        let mut rng = crate::rng::derive(9, b"reinject-overlap");
        let achieved = reinject(&mut values, &scan, None, &mut rng, |_, _| false);
        assert_eq!(achieved[&AnomalyKind::WhitespacePadding], 8);
        assert_eq!(achieved[&AnomalyKind::EncodingArtifact], 8);

        let probe = column("value", ColumnType::Text, values);
        let profile = profile_for(&probe, SemanticType::FreeText);
        let scan_after = detect(&probe, &profile);
        let padding: BTreeSet<usize> = scan_after
            .positions
            .get(&AnomalyKind::WhitespacePadding)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let encoding: BTreeSet<usize> = scan_after
            .positions
            .get(&AnomalyKind::EncodingArtifact)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let both: BTreeSet<usize> = (0..8).collect();
        assert_eq!(
            padding, both,
            "every originally flagged row must still carry the padding defect"
        );
        assert_eq!(
            encoding, both,
            "every originally flagged row must still carry the encoding defect"
        );
    }

    #[test]
    fn reinject_is_deterministic_given_the_same_seed() {
        let (_, baseline) = baseline_for(AnomalyKind::MalformedName);
        let mut extended: Vec<Value> = baseline.clone();
        extended.extend((0..20).map(|i| Value::Text(format!("Pessoa Numero{i}"))));
        let scan = scan_for(AnomalyKind::MalformedName, &[0, 1]);

        let mut a = extended.clone();
        let mut rng_a = crate::rng::derive(3, b"reinject-determinism");
        let achieved_a = reinject(&mut a, &scan, None, &mut rng_a, |_, _| false);

        let mut b = extended;
        let mut rng_b = crate::rng::derive(3, b"reinject-determinism");
        let achieved_b = reinject(&mut b, &scan, None, &mut rng_b, |_, _| false);

        assert_eq!(achieved_a, achieved_b);
        assert_eq!(a, b);
    }

    fn kinds_at_row_zero(values: Vec<Value>, semantic_type: SemanticType) -> Vec<AnomalyKind> {
        let probe = column("value", column_type_for(&values[0]), values);
        let profile = profile_for(&probe, semantic_type);
        detect(&probe, &profile)
            .positions
            .iter()
            .filter(|(_, positions)| positions.contains(&0))
            .map(|(&k, _)| k)
            .collect()
    }

    /// Every value [`AnomalyKind::corrupt_avoiding`] can fall back on must
    /// read as its kind alone, exactly as a draw from
    /// [`AnomalyKind::corrupt`] does, or the anomaly counts the pipeline
    /// promises to preserve would drift whenever a fallback is taken.
    #[test]
    fn every_corrupt_alternative_round_trips_as_exactly_its_kind() {
        for &kind in AnomalyKind::ALL {
            let (semantic_type, baseline) = baseline_for(kind);
            for candidate in candidates_for(kind) {
                for alternative in kind.alternatives(&candidate) {
                    assert_ne!(
                        alternative, candidate,
                        "alternative of {kind:?} on {candidate:?} must change the value"
                    );
                    let mut values = baseline.clone();
                    values[0] = alternative.clone();
                    let hits = kinds_at_row_zero(values, semantic_type);
                    assert_eq!(
                        hits,
                        vec![kind],
                        "alternative {alternative:?} of {kind:?} on {candidate:?} must detect \
                         as {kind:?} alone, got {hits:?}"
                    );
                }
            }
        }
    }

    /// Whatever `reproduces_source` decides, `corrupt_avoiding` draws from
    /// the stream exactly as `corrupt` does, so a column's later positions
    /// land where they would have; and when nothing is rejected it returns
    /// exactly `corrupt`'s value.
    #[test]
    fn corrupt_avoiding_matches_corrupt_when_nothing_is_rejected() {
        for &kind in AnomalyKind::ALL {
            let (semantic_type, baseline) = baseline_for(kind);
            let norm = norm_for(semantic_type, &baseline);
            for candidate in candidates_for(kind) {
                for seed in 0..25u64 {
                    let mut plain_rng = crate::rng::derive(seed, b"corrupt-avoiding");
                    let plain = kind.corrupt(candidate.clone(), norm, &mut plain_rng);

                    let mut accepting_rng = crate::rng::derive(seed, b"corrupt-avoiding");
                    let accepted =
                        kind.corrupt_avoiding(candidate.clone(), norm, &mut accepting_rng, |_| {
                            false
                        });
                    assert_eq!(accepted, plain, "seed {seed}: {kind:?} on {candidate:?}");

                    let mut rejecting_rng = crate::rng::derive(seed, b"corrupt-avoiding");
                    let _ =
                        kind.corrupt_avoiding(candidate.clone(), norm, &mut rejecting_rng, |c| {
                            *c == plain
                        });

                    let next = plain_rng.random::<u64>();
                    assert_eq!(accepting_rng.random::<u64>(), next, "seed {seed}: {kind:?}");
                    assert_eq!(rejecting_rng.random::<u64>(), next, "seed {seed}: {kind:?}");
                }
            }
        }
    }

    fn same_text_ignoring_case(a: &Value, b: &Value) -> bool {
        match (a.as_text(), b.as_text()) {
            (Some(a), Some(b)) => a.to_lowercase() == b.to_lowercase(),
            _ => false,
        }
    }

    #[test]
    fn reinject_never_writes_a_rejected_name() {
        let positions: Vec<usize> = (0..30).step_by(3).collect();
        let source: Vec<Value> = (0..30)
            .map(|row| {
                let name = if row % 3 == 0 {
                    "Tiago Ribeiro1"
                } else {
                    "Paula Nunes"
                };
                Value::Text(name.to_owned())
            })
            .collect();
        let scan = scan_for(AnomalyKind::MalformedName, &positions);

        for seed in 0..200u64 {
            let mut values: Vec<Value> = (0..30)
                .map(|_| Value::Text("Tiago Ribeiro".to_owned()))
                .collect();
            let mut rng = crate::rng::derive(seed, b"reinject-rejected-name");
            let achieved = reinject(&mut values, &scan, None, &mut rng, |row, candidate| {
                same_text_ignoring_case(&source[row], candidate)
            });
            assert_eq!(achieved[&AnomalyKind::MalformedName], positions.len());

            for &position in &positions {
                assert!(
                    !same_text_ignoring_case(&source[position], &values[position]),
                    "seed {seed}: row {position} reproduced its source"
                );
            }
            let probe = column("value", ColumnType::Text, values);
            let profile = profile_for(&probe, SemanticType::FullName);
            assert_eq!(
                detect(&probe, &profile)
                    .counts()
                    .get(&AnomalyKind::MalformedName),
                Some(&positions.len()),
                "seed {seed}"
            );
        }
    }

    fn int_column(values: &[i64]) -> ColumnData {
        column(
            "document",
            ColumnType::Integer,
            values.iter().copied().map(Value::Int).collect(),
        )
    }

    fn malformed_document_rows(values: &[i64], semantic_type: SemanticType) -> Vec<usize> {
        scan_of(&int_column(values), semantic_type)
            .positions
            .get(&AnomalyKind::MalformedDocument)
            .cloned()
            .unwrap_or_default()
    }

    /// `base` followed by its check digits under `wrap`'s rule, as the
    /// integer an integer column stores it as: without leading zeros.
    fn integer_document(base: &[u32], wrap: bool) -> i64 {
        let mut digits = base.to_vec();
        for _ in 0..2 {
            let check = crate::classify::mod11_check_digit(&digits, wrap);
            digits.push(check);
        }
        digits.iter().fold(0i64, |acc, &d| acc * 10 + i64::from(d))
    }

    /// Valid documents of `semantic_type` stored as integers, including some
    /// whose leading zeros integer storage dropped.
    fn integer_documents(semantic_type: SemanticType) -> Vec<i64> {
        let bases: &[&[u32]] = match semantic_type {
            SemanticType::Cnpj => &[
                &[1, 1, 2, 2, 2, 3, 3, 3, 0, 0, 0, 1],
                &[0, 4, 2, 5, 2, 0, 1, 1, 0, 0, 0, 1],
                &[0, 0, 0, 0, 0, 1, 9, 1, 0, 0, 0, 1],
            ],
            _ => &[
                &[1, 1, 1, 4, 4, 4, 7, 7, 7],
                &[0, 1, 2, 3, 4, 5, 6, 7, 8],
                &[0, 0, 3, 4, 5, 6, 7, 8, 9],
            ],
        };
        bases
            .iter()
            .map(|base| integer_document(base, semantic_type == SemanticType::Cnpj))
            .collect()
    }

    const INTEGER_DOCUMENT_TYPES: [SemanticType; 2] = [SemanticType::Cpf, SemanticType::Cnpj];

    #[test]
    fn an_integer_document_with_a_bad_check_digit_is_malformed_document() {
        assert_eq!(
            malformed_document_rows(&[11_144_477_735, 11_144_477_736], SemanticType::Cpf),
            vec![1]
        );
        assert_eq!(
            malformed_document_rows(
                &[11_222_333_000_181, 11_222_333_000_182],
                SemanticType::Cnpj
            ),
            vec![1]
        );
    }

    #[test]
    fn valid_integer_documents_are_never_malformed_including_dropped_leading_zeros() {
        for semantic_type in INTEGER_DOCUMENT_TYPES {
            let documents = integer_documents(semantic_type);
            assert!(
                documents.iter().any(|&n| n < 10_000_000_000),
                "the fixture must hold a document whose leading zeros were dropped"
            );
            assert_eq!(
                malformed_document_rows(&documents, semantic_type),
                Vec::<usize>::new(),
                "{semantic_type:?}: {documents:?}"
            );
        }
    }

    #[test]
    fn negative_zero_and_oversized_integers_in_a_document_column_are_malformed() {
        assert_eq!(
            malformed_document_rows(
                &[
                    11_144_477_735,
                    -11_144_477_735,
                    0,
                    11_222_333_000_181,
                    111_444_777_350
                ],
                SemanticType::Cpf
            ),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            malformed_document_rows(
                &[
                    11_222_333_000_181,
                    -11_222_333_000_181,
                    0,
                    112_223_330_001_810
                ],
                SemanticType::Cnpj
            ),
            vec![1, 2, 3]
        );
    }

    fn hits_at_row_zero(value: Value, semantic_type: SemanticType) -> Vec<AnomalyKind> {
        let mut values: Vec<Value> = integer_documents(semantic_type)
            .into_iter()
            .map(Value::Int)
            .collect();
        values.insert(0, value);
        kinds_at_row_zero(values, semantic_type)
    }

    #[test]
    fn integer_document_corruption_round_trips_as_exactly_malformed_document() {
        let kind = AnomalyKind::MalformedDocument;
        for source_type in INTEGER_DOCUMENT_TYPES {
            for n in integer_documents(source_type) {
                let candidate = Value::Int(n);
                let alternatives = kind.alternatives(&candidate);
                for seed in 0..25u64 {
                    let mut rng = crate::rng::derive(seed, b"integer-document-round-trip");
                    let corrupted = kind.corrupt(candidate.clone(), None, &mut rng);
                    assert!(
                        matches!(corrupted, Value::Int(_)),
                        "seed {seed}: {n} corrupted to {corrupted:?}, not an integer"
                    );
                    assert_ne!(corrupted, candidate, "seed {seed}: {n} came back unchanged");
                    assert!(
                        alternatives.contains(&corrupted),
                        "seed {seed}: {corrupted:?} is not among the alternatives of {n}"
                    );
                    for column_type in INTEGER_DOCUMENT_TYPES {
                        assert_eq!(
                            hits_at_row_zero(corrupted.clone(), column_type),
                            vec![kind],
                            "seed {seed}: {corrupted:?} from {n} in a {column_type:?} column"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_integer_document_alternative_round_trips_as_exactly_malformed_document() {
        let kind = AnomalyKind::MalformedDocument;
        for source_type in INTEGER_DOCUMENT_TYPES {
            for n in integer_documents(source_type) {
                let alternatives = kind.alternatives(&Value::Int(n));
                assert!(!alternatives.is_empty(), "{n} has no alternatives");
                for alternative in alternatives {
                    assert_ne!(alternative, Value::Int(n));
                    for column_type in INTEGER_DOCUMENT_TYPES {
                        assert_eq!(
                            hits_at_row_zero(alternative.clone(), column_type),
                            vec![kind],
                            "{alternative:?} from {n} in a {column_type:?} column"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn integer_document_corruption_never_panics_and_always_reads_as_malformed() {
        for n in [
            i64::MIN,
            i64::MIN + 1,
            -10,
            -1,
            0,
            1,
            9,
            10,
            i64::MAX - 1,
            i64::MAX,
        ] {
            let corrupted = AnomalyKind::MalformedDocument.corrupt(
                Value::Int(n),
                None,
                &mut crate::rng::derive(1, b"integer-document-extremes"),
            );
            let Value::Int(m) = corrupted else {
                panic!("{n} corrupted to {corrupted:?}, not an integer");
            };
            assert_ne!(m, n, "{n} came back unchanged");
            assert_eq!(m < 0, n < 0, "{n} corrupted to {m}, changing its sign");
            assert_eq!(
                m.unsigned_abs().to_string().len(),
                n.unsigned_abs().to_string().len(),
                "{n} corrupted to {m}, changing its width"
            );
            for column_type in INTEGER_DOCUMENT_TYPES {
                assert!(
                    !crate::classify::is_valid_integer_document(column_type, m),
                    "{n} corrupted to {m}, a valid {column_type:?}"
                );
            }
        }
    }

    #[test]
    fn corrupt_avoiding_on_an_integer_document_draws_exactly_like_corrupt() {
        let kind = AnomalyKind::MalformedDocument;
        for source_type in INTEGER_DOCUMENT_TYPES {
            for n in integer_documents(source_type) {
                let candidate = Value::Int(n);
                for seed in 0..25u64 {
                    let mut plain_rng = crate::rng::derive(seed, b"integer-corrupt-avoiding");
                    let plain = kind.corrupt(candidate.clone(), None, &mut plain_rng);

                    let mut accepting_rng = crate::rng::derive(seed, b"integer-corrupt-avoiding");
                    let accepted =
                        kind.corrupt_avoiding(candidate.clone(), None, &mut accepting_rng, |_| {
                            false
                        });
                    assert_eq!(accepted, plain, "seed {seed}: {n}");

                    let mut rejecting_rng = crate::rng::derive(seed, b"integer-corrupt-avoiding");
                    let avoided =
                        kind.corrupt_avoiding(candidate.clone(), None, &mut rejecting_rng, |c| {
                            *c == plain
                        });
                    assert_ne!(avoided, plain, "seed {seed}: {n} kept the rejected draw");

                    let next = plain_rng.random::<u64>();
                    assert_eq!(accepting_rng.random::<u64>(), next, "seed {seed}: {n}");
                    assert_eq!(rejecting_rng.random::<u64>(), next, "seed {seed}: {n}");
                }
            }
        }
    }

    #[test]
    fn reinject_never_writes_a_rejected_integer_document() {
        let documents = integer_documents(SemanticType::Cpf);
        let anonymized: Vec<Value> = (0..30)
            .map(|row| Value::Int(documents[row % documents.len()]))
            .collect();
        let positions: Vec<usize> = (0..30).step_by(3).collect();
        let scan = scan_for(AnomalyKind::MalformedDocument, &positions);

        for seed in 0..100u64 {
            let mut drawn = anonymized.clone();
            let mut rng = crate::rng::derive(seed, b"reinject-rejected-integer-document");
            reinject(&mut drawn, &scan, None, &mut rng, |_, _| false);
            let source = drawn.clone();

            let mut values = anonymized.clone();
            let mut rng = crate::rng::derive(seed, b"reinject-rejected-integer-document");
            let achieved = reinject(&mut values, &scan, None, &mut rng, |row, candidate| {
                source[row] == *candidate
            });
            assert_eq!(achieved[&AnomalyKind::MalformedDocument], positions.len());
            for &position in &positions {
                assert_ne!(
                    values[position], source[position],
                    "seed {seed}: row {position} reproduced its source"
                );
            }
            let probe = column("value", ColumnType::Integer, values);
            assert_eq!(
                scan_of(&probe, SemanticType::Cpf).positions,
                BTreeMap::from([(AnomalyKind::MalformedDocument, positions.clone())]),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn reinject_on_integer_documents_is_deterministic_given_the_same_seed() {
        let documents = integer_documents(SemanticType::Cnpj);
        let values: Vec<Value> = (0..20)
            .map(|row| Value::Int(documents[row % documents.len()]))
            .collect();
        let scan = scan_for(AnomalyKind::MalformedDocument, &[0, 4, 9]);

        let mut a = values.clone();
        let mut rng_a = crate::rng::derive(3, b"reinject-integer-determinism");
        let achieved_a = reinject(&mut a, &scan, None, &mut rng_a, |_, _| false);

        let mut b = values;
        let mut rng_b = crate::rng::derive(3, b"reinject-integer-determinism");
        let achieved_b = reinject(&mut b, &scan, None, &mut rng_b, |_, _| false);

        assert_eq!(achieved_a, achieved_b);
        assert_eq!(a, b);
    }

    fn reinject_sentinels_avoiding(rejected: &[&str], seed: u64) -> Vec<Value> {
        let mut values: Vec<Value> = (0..40)
            .map(|i| Value::Text(format!("clean sentence number {i}")))
            .collect();
        let positions: Vec<usize> = (0..40).step_by(4).collect();
        let scan = scan_for(AnomalyKind::UnexpectedNull, &positions);
        let mut rng = crate::rng::derive(seed, b"reinject-rejected-sentinel");
        let achieved = reinject(&mut values, &scan, None, &mut rng, |_, candidate| {
            candidate
                .as_text()
                .is_some_and(|text| rejected.contains(&text))
        });
        assert_eq!(achieved[&AnomalyKind::UnexpectedNull], positions.len());

        let probe = column("value", ColumnType::Text, values.clone());
        let profile = profile_for(&probe, SemanticType::FreeText);
        let scan_after = detect(&probe, &profile);
        assert_eq!(
            scan_after.positions.get(&AnomalyKind::UnexpectedNull),
            Some(&positions),
            "seed {seed}"
        );
        assert_eq!(scan_after.positions.len(), 1, "seed {seed}");
        positions
            .iter()
            .map(|&position| values[position].clone())
            .collect()
    }

    #[test]
    fn reinject_never_writes_a_rejected_sentinel_and_falls_back_to_empty() {
        for seed in 0..50u64 {
            let written = reinject_sentinels_avoiding(NULL_SENTINELS, seed);
            assert!(
                written
                    .iter()
                    .all(|value| *value == Value::Text(EMPTY_NULL_SENTINEL.to_owned())),
                "seed {seed}: {written:?}"
            );

            let written = reinject_sentinels_avoiding(&["n/a"], seed);
            assert!(
                written.iter().all(|value| {
                    value
                        .as_text()
                        .is_some_and(|text| text != "n/a" && NULL_SENTINELS.contains(&text))
                }),
                "seed {seed}: {written:?}"
            );
        }
    }

    fn shapes(of: &[NameShape]) -> BTreeSet<NameShape> {
        of.iter().copied().collect()
    }

    fn every_name_shape_set() -> Vec<BTreeSet<NameShape>> {
        use NameShape::{AllLowercase, AllUppercase, EmbeddedDigit};
        vec![
            shapes(&[AllUppercase]),
            shapes(&[AllLowercase]),
            shapes(&[EmbeddedDigit]),
            shapes(&[AllUppercase, EmbeddedDigit]),
            shapes(&[AllLowercase, EmbeddedDigit]),
        ]
    }

    fn name_shapes_at_row_zero(values: Vec<Value>) -> Option<BTreeSet<NameShape>> {
        let probe = column("value", ColumnType::Text, values);
        scan_of(&probe, SemanticType::FullName)
            .name_shapes
            .get(&0)
            .cloned()
    }

    #[test]
    fn name_shapes_tell_all_caps_all_lowercase_and_embedded_digit_apart() {
        use NameShape::{AllLowercase, AllUppercase, EmbeddedDigit};
        let column = text_column(
            "name",
            &[
                "Ana Souza",
                "BRUNO LIMA",
                "carla dias",
                "D4vi Reis",
                "ANA1",
                "ana1",
                "1234",
            ],
        );
        let scan = scan_of(&column, SemanticType::FullName);
        assert_eq!(
            scan.name_shapes,
            BTreeMap::from([
                (1, shapes(&[AllUppercase])),
                (2, shapes(&[AllLowercase])),
                (3, shapes(&[EmbeddedDigit])),
                (4, shapes(&[AllUppercase, EmbeddedDigit])),
                (5, shapes(&[AllLowercase, EmbeddedDigit])),
                (6, shapes(&[EmbeddedDigit])),
            ])
        );
        let shaped_rows: Vec<usize> = scan.name_shapes.keys().copied().collect();
        assert_eq!(shaped_rows, scan.positions[&AnomalyKind::MalformedName]);
    }

    #[test]
    fn every_name_shape_set_round_trips_as_exactly_those_shapes() {
        let (_, baseline) = baseline_for(AnomalyKind::MalformedName);
        for set in every_name_shape_set() {
            for candidate in ["Ana Souza", "Bruno Lima"] {
                let shaped = name_in_shapes(candidate, &set, '1');
                assert_ne!(shaped, candidate, "{set:?} on {candidate:?}");
                let mut values = baseline.clone();
                values[0] = Value::Text(shaped.clone());
                assert_eq!(
                    kinds_at_row_zero(values.clone(), SemanticType::FullName),
                    vec![AnomalyKind::MalformedName],
                    "{shaped:?}"
                );
                assert_eq!(
                    name_shapes_at_row_zero(values),
                    Some(set.clone()),
                    "{shaped:?}"
                );
            }

            let mut values = baseline.clone();
            values[0] = Value::Text(name_in_shapes("中文", &set, '1'));
            assert_eq!(
                name_shapes_at_row_zero(values),
                Some(shapes(&[NameShape::EmbeddedDigit])),
                "{set:?} on a name with no ASCII letter"
            );
        }
    }

    #[test]
    fn every_shaped_name_alternative_round_trips_as_malformed_name_alone() {
        let (_, baseline) = baseline_for(AnomalyKind::MalformedName);
        for set in every_name_shape_set() {
            for candidate in ["Ana Souza", "Bruno Lima", "中文"] {
                for alternative in shaped_name_alternatives(candidate, &set) {
                    assert_ne!(alternative, candidate, "{set:?} on {candidate:?}");
                    let mut values = baseline.clone();
                    values[0] = Value::Text(alternative.clone());
                    assert_eq!(
                        kinds_at_row_zero(values, SemanticType::FullName),
                        vec![AnomalyKind::MalformedName],
                        "{alternative:?} of {set:?} on {candidate:?}"
                    );
                }
            }
        }
    }

    fn mixed_malformed_names() -> ColumnData {
        text_column(
            "name",
            &[
                "Ana Souza",
                "BRUNO LIMA",
                "carla dias",
                "D4vi Reis",
                "ELISA COSTA1",
                "fabio nunes2",
                "Gabriel Rocha",
                "HELENA MARTINS",
                "Igor Alves",
                "julia campos",
            ],
        )
    }

    fn title_case_names(count: usize) -> Vec<Value> {
        const NAMES: &[&str] = &[
            "Paula Nunes",
            "Tiago Ribeiro",
            "Renata Lopes",
            "Marcos Teixeira",
        ];
        (0..count)
            .map(|row| Value::Text(NAMES[row % NAMES.len()].to_owned()))
            .collect()
    }

    #[test]
    fn reinject_keeps_each_rows_recorded_name_shape() {
        let source = mixed_malformed_names();
        let scan = scan_of(&source, SemanticType::FullName);
        for seed in 0..20u64 {
            let mut values = title_case_names(source.values.len());
            let mut rng = crate::rng::derive(seed, b"reinject-name-shapes");
            let achieved = reinject(&mut values, &scan, None, &mut rng, |_, _| false);
            assert_eq!(
                achieved[&AnomalyKind::MalformedName],
                scan.positions[&AnomalyKind::MalformedName].len()
            );

            let after = scan_of(
                &column("name", ColumnType::Text, values),
                SemanticType::FullName,
            );
            assert_eq!(after.positions, scan.positions, "seed {seed}");
            assert_eq!(after.name_shapes, scan.name_shapes, "seed {seed}");
        }
    }

    #[test]
    fn reinject_on_recorded_name_shapes_draws_nothing_from_the_stream() {
        let source = mixed_malformed_names();
        let scan = scan_of(&source, SemanticType::FullName);
        assert_eq!(
            scan.positions.len(),
            1,
            "the fixture holds malformed names alone"
        );
        for seed in 0..20u64 {
            let mut values = title_case_names(source.values.len());
            let mut rng = crate::rng::derive(seed, b"reinject-name-shapes-stream");
            reinject(&mut values, &scan, None, &mut rng, |_, _| false);

            let mut untouched = crate::rng::derive(seed, b"reinject-name-shapes-stream");
            assert_eq!(
                rng.random::<u64>(),
                untouched.random::<u64>(),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn a_colliding_shaped_name_falls_back_without_reproducing_its_source() {
        let (_, baseline) = baseline_for(AnomalyKind::MalformedName);

        let caps_source = Value::Text("TIAGO RIBEIRO".to_owned());
        let written = corrupt_name_in_shapes_avoiding(
            Value::Text("Tiago Ribeiro".to_owned()),
            &shapes(&[NameShape::AllUppercase]),
            |candidate| same_text_ignoring_case(&caps_source, candidate),
        );
        assert!(
            !same_text_ignoring_case(&caps_source, &written),
            "{written:?} reproduced its source"
        );
        let mut values = baseline.clone();
        values[0] = written.clone();
        assert_eq!(
            kinds_at_row_zero(values, SemanticType::FullName),
            vec![AnomalyKind::MalformedName],
            "{written:?}"
        );

        let digit_source = Value::Text("Tiago Ribeiro1".to_owned());
        let written = corrupt_name_in_shapes_avoiding(
            Value::Text("Tiago Ribeiro".to_owned()),
            &shapes(&[NameShape::EmbeddedDigit]),
            |candidate| same_text_ignoring_case(&digit_source, candidate),
        );
        assert_eq!(written, Value::Text("Tiago Ribeiro2".to_owned()));
        let mut values = baseline;
        values[0] = written;
        assert_eq!(
            name_shapes_at_row_zero(values),
            Some(shapes(&[NameShape::EmbeddedDigit]))
        );
    }
}
