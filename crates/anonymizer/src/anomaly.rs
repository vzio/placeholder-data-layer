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

/// Fixed substrings that mark classic UTF-8-decoded-as-Latin-1 corruption on
/// pt-BR text (`café` -> `cafÃ©`). Not exhaustive, but these are the sequences
/// that actually recur in production dumps, which is the bar this detector
/// has to clear.
const MOJIBAKE_MARKERS: &[&str] = &[
    "Ã©", "Ã¡", "Ã¢", "Ã£", "Ã§", "Ã³", "Ã´", "Ãº", "Ã\u{ad}", "Â«", "Â»", "â€™", "â€œ",
];

/// Classes of production corruption the engine can both detect and
/// synthesise. Enum dispatch: adding a kind is a variant plus an arm in
/// [`detect`] and in [`AnomalyKind::corrupt`], never a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyKind {
    MalformedEmail,
    MalformedDocument,
    MalformedPhone,
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
    /// class, chosen independently of whatever source value originally
    /// triggered detection.
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
    pub fn corrupt(self, value: Value, norm: Option<LengthStats>, rng: &mut ChaCha8Rng) -> Value {
        match self {
            AnomalyKind::MalformedEmail => with_text(value, |text| insert_at(text, ' ', rng)),
            AnomalyKind::MalformedPhone | AnomalyKind::MalformedDocument => {
                with_text(value, |text| insert_at(text, 'x', rng))
            }
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
}

/// Positions, per kind, of the anomalies found in one column. Positions are
/// row indices, never values: an index cannot be production PII, which is
/// what lets this type stay unexported from the report while still being
/// freely computable and freely tested.
#[derive(Debug, Clone, Default)]
pub struct AnomalyScan {
    pub positions: BTreeMap<AnomalyKind, Vec<usize>>,
}

impl AnomalyScan {
    pub fn counts(&self) -> BTreeMap<AnomalyKind, usize> {
        self.positions.iter().map(|(k, v)| (*k, v.len())).collect()
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

    for (row, value) in column.values.iter().enumerate() {
        match value {
            Value::Text(text) => detect_text(profile, text, row, &mut positions),
            Value::Int(n) => detect_numeric(profile, *n as f64, row, &mut positions),
            Value::Float(f) => detect_numeric(profile, *f, row, &mut positions),
            Value::Null | Value::Bool(_) => {}
        }
    }

    AnomalyScan { positions }
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

/// Chooses `floor(rate * n)` distinct positions out of `0..n` by a seeded
/// shuffle, so the same rate always yields the same count: an equality the
/// corresponding functional test can assert directly, rather than a
/// tolerance band around an independent per-row coin flip at that rate.
/// Deterministic given `rng`'s state, and therefore independent of thread
/// count as long as `rng` itself was derived per-column, never shared.
pub fn exact_count_positions(rate: f64, n: usize, rng: &mut ChaCha8Rng) -> BTreeSet<usize> {
    let amount = exact_count(rate, n);
    partial_shuffle_positions(n, amount, rng)
        .into_iter()
        .collect()
}

/// Selects `amount` distinct positions out of `0..n` by a partial
/// Fisher-Yates shuffle, implemented here rather than delegating to
/// `rand::seq::index::sample`: that function's own documentation disclaims
/// a stable selection algorithm ("the implementation used is not
/// specified"), so a `cargo update` within the caret-constrained `rand`
/// dependency could change which positions a pinned seed produces. Rolling
/// the shuffle here keeps the guarantee pinned to this crate's source
/// rather than to whichever `rand` release happens to be resolved.
fn partial_shuffle_positions(n: usize, amount: usize, rng: &mut ChaCha8Rng) -> Vec<usize> {
    let mut pool: Vec<usize> = (0..n).collect();
    for i in 0..amount {
        let j = i + rng.random_range(0..n - i);
        pool.swap(i, j);
    }
    pool.truncate(amount);
    pool
}

fn exact_count(rate: f64, n: usize) -> usize {
    if n == 0 || rate <= 0.0 {
        return 0;
    }
    ((rate * n as f64).floor() as usize).min(n)
}

/// Re-injects each kind at exactly `floor(rate * n)` positions, chosen by
/// [`exact_count_positions`], so input and output rates match exactly.
pub fn reinject(
    _values: &mut [Value],
    _scan: &AnomalyScan,
    _norm: Option<LengthStats>,
    _rng: &mut ChaCha8Rng,
) -> BTreeMap<AnomalyKind, usize> {
    todo!("P4: drive exact_count_positions + AnomalyKind::corrupt per kind")
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
        SemanticType::FullName if has_name_formatting_issue(trimmed) => {
            record(positions, AnomalyKind::MalformedName, row);
        }
        SemanticType::Date if is_impossible_date(trimmed) => {
            record(positions, AnomalyKind::ImpossibleDate, row);
        }
        _ => {}
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
        SemanticType::Cpf => classify::is_valid_cpf(text),
        SemanticType::Cnpj => classify::is_valid_cnpj(text),
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

/// All-uppercase, all-lowercase, or carrying an embedded digit: the three
/// shapes a person's name recurrently takes in a dump that came from a form
/// with no server-side validation.
fn has_name_formatting_issue(text: &str) -> bool {
    if text.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    let letters: Vec<char> = text.chars().filter(char::is_ascii_alphabetic).collect();
    if letters.is_empty() {
        return false;
    }
    letters.iter().all(|c| c.is_uppercase()) || letters.iter().all(|c| c.is_lowercase())
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
fn parse_date_fields(text: &str) -> Option<(u32, u32, u32)> {
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

fn days_in_month(year: u32, month: u32) -> u32 {
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
    let mut chars: Vec<char> = text.chars().collect();
    let position = if chars.len() >= 2 {
        rng.random_range(1..chars.len())
    } else {
        chars.len()
    };
    chars.insert(position, ch);
    chars.into_iter().collect()
}

/// The casing arms are only a corruption when `text` has a cased ASCII
/// letter for [`has_name_formatting_issue`] to notice; a value with none
/// (`"中文"`) passes through both unchanged, so they are only drawn when one
/// exists. Otherwise the digit-append arm runs unconditionally, which always
/// changes the value and always introduces a digit that check flags.
fn corrupt_name(text: &str, rng: &mut ChaCha8Rng) -> String {
    if !text.chars().any(|c| c.is_ascii_alphabetic()) {
        return format!("{text}1");
    }
    let choice: u8 = rng.random_range(0..3);
    match choice {
        0 => text.to_uppercase(),
        1 => text.to_lowercase(),
        _ => format!("{text}1"),
    }
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
    match choice {
        0 => format!("  {text}"),
        1 => format!("{text}  "),
        _ => format!("{text}\t"),
    }
}

fn corrupt_encoding(text: &str, rng: &mut ChaCha8Rng) -> String {
    if rng.random_bool(0.5) {
        format!("{text}\u{FFFD}")
    } else {
        format!("{text}{}", MOJIBAKE_MARKERS[0])
    }
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

#[cfg(test)]
mod tests {
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

    fn rng() -> ChaCha8Rng {
        crate::rng::derive(1, b"test")
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

    #[test]
    fn exact_count_positions_matches_floor_rate_times_n() {
        let mut rng = rng();
        let positions = exact_count_positions(0.2, 50, &mut rng);
        assert_eq!(positions.len(), 10);
        assert!(positions.iter().all(|&p| p < 50));
    }

    #[test]
    fn exact_count_positions_is_deterministic_given_the_same_seed() {
        let positions_a = exact_count_positions(0.37, 211, &mut crate::rng::derive(99, b"col"));
        let positions_b = exact_count_positions(0.37, 211, &mut crate::rng::derive(99, b"col"));
        assert_eq!(positions_a, positions_b);
    }

    #[test]
    fn exact_count_positions_never_exceeds_n_even_when_rate_rounds_up() {
        let mut rng = rng();
        let positions = exact_count_positions(1.0, 5, &mut rng);
        assert_eq!(positions.len(), 5);
    }

    #[test]
    fn exact_count_positions_on_an_empty_column_is_empty() {
        let mut rng = rng();
        assert!(exact_count_positions(0.5, 0, &mut rng).is_empty());
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

    /// Regression for the breakdown point of mean and standard deviation: a
    /// handful of injected outliers among `n` values used to poison the very
    /// statistic meant to find them, so a corrupted column of this shape
    /// detected none of them. Median and median absolute deviation have a
    /// 50% breakdown point and must catch every one at this injection rate.
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

    /// Pinned against a hand-rolled partial Fisher-Yates shuffle rather than
    /// `rand::seq::index::sample`, whose own documentation disclaims a
    /// stable selection algorithm across `rand` releases.
    #[test]
    fn partial_shuffle_positions_matches_a_pinned_expected_set() {
        let mut rng = crate::rng::derive(42, b"pinned-shuffle");
        let positions = partial_shuffle_positions(10, 4, &mut rng);
        assert_eq!(positions, vec![9, 7, 2, 3]);
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
            // "中文" has no cased ASCII letter, the regression covered by
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
}
