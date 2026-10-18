use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::dataset::Value;

/// How many leading non-null values of a column are inspected for shape
/// evidence. Bounded and deterministic: always the first N in column order,
/// never a random draw, so classification cannot depend on a seed.
const SAMPLE_LIMIT: usize = 64;

/// A name-only guess carries less weight than a single confirmed value, since
/// it is never contradicted by data that might say otherwise.
const NAME_ONLY_CONFIDENCE: f32 = 0.3;

/// Default confidence assigned to a text column that matches no structured
/// pattern. Not zero: failing every detector is itself mild evidence that
/// the column is unstructured prose rather than something unrecognized.
const FREE_TEXT_CONFIDENCE: f32 = 0.6;

/// Minimum sampled match rate for a structured type to win a column. A
/// column is many values, and one coincidental match among dozens of
/// unrelated ones is not evidence that the column holds that type — the
/// values sampled must actually agree on it.
const STRUCTURED_TYPE_MAJORITY: f32 = 0.5;

static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    // A fixed literal, never built from runtime data: this can only fail to
    // compile if the pattern itself has a bug, which a single test run
    // catches immediately.
    Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").expect("email pattern is valid")
});

static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\+?55\s?)?\(?\d{2}\)?[\s.-]?\d{4,5}[\s.-]?\d{4}$")
        .expect("phone pattern is valid")
});

static CEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{5}-?\d{3}$").expect("cep pattern is valid"));

static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2}|\d{2}[/-]\d{2}[/-]\d{4})$").expect("date pattern is valid")
});

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$|^[0-9a-fA-F]{32}$",
    )
    .expect("uuid pattern is valid")
});

/// The 8-4-4-4-12 hyphenated form or the 32-character bare form, hex digits
/// only, case-insensitive. Structural only: this does not check the version
/// or variant nibbles a real UUID's bit layout would carry, since a column
/// holding `gen_random_uuid()` output and a hand-rolled legacy identifier of
/// the same shape must be treated alike for identity purposes.
pub(crate) fn is_uuid_shaped(text: &str) -> bool {
    UUID_RE.is_match(text)
}

/// What a column means, as far as this build can tell. Enum dispatch: adding a
/// detector is a variant plus an arm, never a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SemanticType {
    Email,
    Uuid,
    Cpf,
    Cnpj,
    Phone,
    Cep,
    /// A login or handle: lowercase, no spaces, often unique-keyed.
    /// Recognised by its role in the schema, not by its shape: a column
    /// name matching a handle hint is *necessary* here, not a confidence
    /// booster the way it is for every other structured type — see
    /// [`column_match_rate`] for why, and [`looks_like_username`] for the
    /// per-value shape check layered on top of the name requirement. An
    /// unnamed handle column (no `login`/`username`/`handle`/… in its
    /// name) is a known, accepted miss, not a bug: shape alone cannot tell
    /// a login apart from a slug or a status enum, so this type is
    /// deliberately narrow rather than broad and wrong.
    Username,
    FullName,
    Date,
    /// Any numeric column not otherwise classified. The name is aspirational:
    /// this build does not distinguish a surrogate key from a plain
    /// measurement such as an age or a price, so both land here.
    NumericId,
    FreeText,
    Unknown,
}

impl SemanticType {
    /// Backs `GET /v1/detectors`.
    pub const ALL: &'static [SemanticType] = &[
        SemanticType::Email,
        SemanticType::Uuid,
        SemanticType::Cpf,
        SemanticType::Cnpj,
        SemanticType::Phone,
        SemanticType::Cep,
        SemanticType::Username,
        SemanticType::FullName,
        SemanticType::Date,
        SemanticType::NumericId,
        SemanticType::FreeText,
        SemanticType::Unknown,
    ];

    /// The patterns scored against sampled values. This array's position
    /// governs exactly one thing: [`classify_by_name_only`] returns the
    /// first hint match it finds while walking this slice in order. A tie
    /// in value-match rate inside [`classify_text`] is decided separately,
    /// by [`SemanticType`]'s derived `Ord` (declaration order) — not by
    /// this array at all. The two orders are kept aligned above as a
    /// matter of not inviting confusion, not because either mechanism
    /// reads the other's ordering.
    const STRUCTURED_TEXT_TYPES: &'static [SemanticType] = &[
        SemanticType::Email,
        SemanticType::Uuid,
        SemanticType::Cpf,
        SemanticType::Cnpj,
        SemanticType::Phone,
        SemanticType::Cep,
        SemanticType::Username,
        SemanticType::FullName,
        SemanticType::Date,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SemanticType::Email => "email",
            SemanticType::Uuid => "uuid",
            SemanticType::Cpf => "cpf",
            SemanticType::Cnpj => "cnpj",
            SemanticType::Phone => "phone",
            SemanticType::Cep => "cep",
            SemanticType::FullName => "full_name",
            SemanticType::Date => "date",
            SemanticType::Username => "username",
            SemanticType::NumericId => "numeric_id",
            SemanticType::FreeText => "free_text",
            SemanticType::Unknown => "unknown",
        }
    }

    /// Direct identifiers are columns whose raw values single out one person
    /// on their own. This gates what the profile and the fidelity report are
    /// allowed to surface: a categorical distribution over a direct
    /// identifier's raw values would leak exactly what the anonymizer exists
    /// to hide.
    pub fn is_direct_identifier(self) -> bool {
        matches!(
            self,
            SemanticType::Email
                | SemanticType::Cpf
                | SemanticType::Cnpj
                | SemanticType::Phone
                | SemanticType::FullName
                | SemanticType::Username
        )
    }

    fn name_hints(self) -> &'static [&'static str] {
        match self {
            SemanticType::Email => &["email", "e-mail", "e_mail", "correio"],
            SemanticType::Uuid => &["uuid", "guid", "uid"],
            SemanticType::Cpf => &["cpf"],
            SemanticType::Cnpj => &["cnpj"],
            SemanticType::Phone => &["phone", "telefone", "celular", "fone", "tel"],
            SemanticType::Cep => &["cep", "zipcode", "zip", "postal"],
            SemanticType::FullName => &["name", "nome", "fullname"],
            SemanticType::Date => &["date", "data", "dt", "nascimento", "birthdate"],
            SemanticType::Username => &["username", "login", "handle", "nickname", "nick"],
            SemanticType::NumericId | SemanticType::FreeText | SemanticType::Unknown => &[],
        }
    }

    /// Shape evidence for `text` against `self`, independent of any column
    /// name or sample-wide statistics. [`crate::anomaly`] reuses this for
    /// most types, so a value judged malformed and a value judged
    /// well-formed agree with how classification itself would have scored
    /// the same text. [`SemanticType::FullName`] and [`SemanticType::Date`]
    /// are the exceptions: their malformed-value checks are deliberately
    /// looser than what this function demands of a plausible shape, so
    /// `"ANA SOUZA"` and a non-leap `"2023-02-29"` are both well-formed here
    /// while anomaly detection flags them.
    pub(crate) fn matches_text(self, text: &str) -> bool {
        match self {
            SemanticType::Email => EMAIL_RE.is_match(text),
            SemanticType::Uuid => is_uuid_shaped(text),
            SemanticType::Cpf => is_valid_cpf(text),
            SemanticType::Cnpj => is_valid_cnpj(text),
            SemanticType::Phone => PHONE_RE.is_match(text),
            SemanticType::Cep => CEP_RE.is_match(text),
            SemanticType::Date => DATE_RE.is_match(text),
            SemanticType::FullName => looks_like_full_name(text),
            SemanticType::Username => looks_like_username(text),
            SemanticType::NumericId | SemanticType::FreeText | SemanticType::Unknown => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub semantic_type: SemanticType,
    /// The sampled match rate for `semantic_type`, nudged toward 1.0 when
    /// the column name agrees. A raw rate, not a calibrated probability: a
    /// single-row sample that matches reports 1.0 same as a thousand-row
    /// one would.
    pub confidence: f32,
}

/// Column name heuristics first, then value-shape evidence from the sample.
/// Value evidence dominates: a name that disagrees with every sampled value
/// never outscores a value-shape match, and a name that agrees only nudges
/// the confidence of a type the values already support.
pub fn classify(name: &str, values: &[Value]) -> Classification {
    let sample: Vec<&Value> = values
        .iter()
        .filter(|value| !value.is_null())
        .take(SAMPLE_LIMIT)
        .collect();

    if sample.is_empty() {
        return classify_by_name_only(name);
    }

    let texts: Vec<&str> = sample.iter().filter_map(|value| value.as_text()).collect();
    if !texts.is_empty() {
        return classify_text(name, &texts);
    }

    let numeric = sample
        .iter()
        .filter(|value| matches!(value, Value::Int(_) | Value::Float(_)))
        .count();
    if numeric == sample.len() {
        if let Some(ints) = all_ints(&sample) {
            if let Some(classification) = classify_int_document_shape(name, &ints) {
                return classification;
            }
        }
        return Classification {
            semantic_type: SemanticType::NumericId,
            confidence: 1.0,
        };
    }

    Classification {
        semantic_type: SemanticType::Unknown,
        confidence: 0.0,
    }
}

fn all_ints(sample: &[&Value]) -> Option<Vec<i64>> {
    sample
        .iter()
        .map(|value| match value {
            Value::Int(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// Checks an all-integer column's values in their decimal string form
/// against the document detectors that only ever run on text — without
/// this, a CPF or CNPJ stored as `BIGINT` is indistinguishable from a
/// surrogate id, since [`Value::as_text`] has nothing to return for
/// [`Value::Int`]. Restricted to [`SemanticType::Cpf`] and
/// [`SemanticType::Cnpj`]: both carry a check digit, so a plain sequential
/// id matching one is a one-in-a-hundred fluke rather than a width
/// coincidence. [`SemanticType::Phone`] and [`SemanticType::Cep`] have no
/// check digit — nothing but a digit count distinguishes them from an
/// ordinary integer id — so they are never scored here; the other
/// structured text types require characters an integer's decimal form
/// cannot contain ([`SemanticType::Email`], [`SemanticType::FullName`]) or
/// are irrelevant to a value this short ([`SemanticType::Uuid`], which an
/// `i64` can never reach the length of).
fn classify_int_document_shape(name: &str, ints: &[i64]) -> Option<Classification> {
    let texts: Vec<String> = ints.iter().map(i64::to_string).collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();

    let scored = [SemanticType::Cpf, SemanticType::Cnpj]
        .iter()
        .map(|&ty| (ty, text_match_rate(ty, &text_refs)))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));

    match scored {
        Some((semantic_type, rate)) if rate >= STRUCTURED_TYPE_MAJORITY => Some(Classification {
            semantic_type,
            confidence: boost(rate, name_hints_match(semantic_type, name)),
        }),
        _ => None,
    }
}

fn classify_text(name: &str, texts: &[&str]) -> Classification {
    // `max_by` returns the *last* maximum on a tie, so the comparator orders
    // by rate first and, on a tie, treats the earlier candidate as the
    // greater one: the one already chosen stays chosen.
    let scored = SemanticType::STRUCTURED_TEXT_TYPES
        .iter()
        .map(|&ty| (ty, column_match_rate(ty, name, texts)))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));

    match scored {
        Some((semantic_type, rate)) if rate >= STRUCTURED_TYPE_MAJORITY => Classification {
            semantic_type,
            confidence: boost(rate, name_hints_match(semantic_type, name)),
        },
        _ => Classification {
            semantic_type: SemanticType::FreeText,
            confidence: FREE_TEXT_CONFIDENCE,
        },
    }
}

/// [`SemanticType::Username`] is excluded from the name-only fallback
/// entirely, never just scored low: this function only runs when every
/// sampled value is null (or there is no sample at all), and a handle
/// column's defining confirmation — [`looks_like_username`] actually
/// matching — requires a real, non-null value to check. Guessing `Username`
/// from a name alone, with nothing to confirm it against, is exactly the
/// shape of bug that let an all-null `last_login`/`nickname` column of a
/// non-`Text` type reach [`crate::transform::Strategy::MintHandle`] and
/// fail with an incompatible-strategy error that `main` never produced for
/// the same column — see `tests/username.rs`'s regression coverage.
fn classify_by_name_only(name: &str) -> Classification {
    for &semantic_type in SemanticType::STRUCTURED_TEXT_TYPES {
        if semantic_type == SemanticType::Username {
            continue;
        }
        if name_hints_match(semantic_type, name) {
            return Classification {
                semantic_type,
                confidence: NAME_ONLY_CONFIDENCE,
            };
        }
    }
    Classification {
        semantic_type: SemanticType::Unknown,
        confidence: 0.0,
    }
}

fn text_match_rate(semantic_type: SemanticType, texts: &[&str]) -> f32 {
    if texts.is_empty() {
        return 0.0;
    }
    let matches = texts
        .iter()
        .filter(|text| semantic_type.matches_text(text))
        .count();
    matches as f32 / texts.len() as f32
}

/// [`text_match_rate`], gated by whatever column-level requirement
/// `semantic_type` carries beyond per-value shape. [`SemanticType::Username`]
/// is the only type with one today: its column name must match a handle
/// hint (`name_hints_match`) — necessary, not merely a confidence booster
/// the way it is for every other type. Shape evidence alone cannot carry
/// `Username` on its own: a slug, a SKU, a file name or a dotted event
/// name can all be exactly as shape-conforming as a real login, and most
/// of those are *more* uniquely-valued than a login column ever is (a
/// generated slug repeats the source title; an auto-incrementing SKU never
/// repeats at all), so no cardinality threshold can separate them — the
/// information needed is not in the characters, only in what the column is
/// called. Zeroing the rate rather than excluding the candidate outright
/// lets an unnamed column still fall through to whichever other type — or
/// `FreeText` — its values actually support.
fn column_match_rate(semantic_type: SemanticType, name: &str, texts: &[&str]) -> f32 {
    if semantic_type == SemanticType::Username && !name_hints_match(semantic_type, name) {
        return 0.0;
    }
    text_match_rate(semantic_type, texts)
}

/// Nudges a value-confirmed score toward 1.0 when the column name agrees,
/// without ever letting the name alone manufacture confidence the values
/// did not earn.
fn boost(value_score: f32, name_agrees: bool) -> f32 {
    if name_agrees {
        (value_score + (1.0 - value_score) * 0.3).min(1.0)
    } else {
        value_score
    }
}

fn name_hints_match(semantic_type: SemanticType, name: &str) -> bool {
    let lower = name.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    semantic_type.name_hints().iter().any(|hint| {
        if hint.len() >= 5 {
            lower.contains(hint)
        } else {
            tokens.contains(hint)
        }
    })
}

fn looks_like_full_name(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 2 {
        return false;
    }
    tokens.iter().all(|token| {
        let mut chars = token.chars();
        chars.next().is_some_and(char::is_uppercase)
            && chars.all(|c| c.is_alphabetic() || c == '\'' || c == '-')
    })
}

/// Starts with a lowercase ASCII letter, continues with lowercase letters,
/// digits, `.`, `_` or `-`, is `3`-`32` characters long, and carries at
/// least one digit or separator. That last condition is what keeps a plain
/// lowercase dictionary word — a status column's `"pending"` or `"active"`
/// — from looking like a login on name-shape alone: a real handle almost
/// always carries a number or a separator somewhere, a status value never
/// does.
pub(crate) fn looks_like_username(text: &str) -> bool {
    let len = text.chars().count();
    if !(3..=32).contains(&len) {
        return false;
    }
    if !text.starts_with(|c: char| c.is_ascii_lowercase()) {
        return false;
    }
    let mut has_digit_or_separator = false;
    for c in text.chars().skip(1) {
        match c {
            'a'..='z' => {}
            '0'..='9' | '.' | '_' | '-' => has_digit_or_separator = true,
            _ => return false,
        }
    }
    has_digit_or_separator
}

fn only_digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

/// Whether `text` holds nothing but digits and characters from
/// `punctuation`: rejects a bare or punctuated document number framed by
/// other junk, which digit-stripping alone would otherwise validate.
fn has_no_stray_characters(text: &str, punctuation: &[char]) -> bool {
    text.chars()
        .all(|c| c.is_ascii_digit() || punctuation.contains(&c))
}

fn all_same_digit(digits: &[u32]) -> bool {
    digits
        .first()
        .is_some_and(|&first| digits.iter().all(|&d| d == first))
}

/// A mod-11 check digit over `base`, weighted from the rightmost digit with
/// weight 2, increasing by one per digit moving left. `wrap` makes the
/// weight fall back to 2 once it passes 9, which is CNPJ's rule; CPF never
/// wraps because its base is short enough that the weight never needs to.
pub(crate) fn mod11_check_digit(base: &[u32], wrap: bool) -> u32 {
    let mut weight = 2u32;
    let mut sum = 0u32;
    for &digit in base.iter().rev() {
        sum += digit * weight;
        weight = if wrap && weight == 9 { 2 } else { weight + 1 };
    }
    let remainder = sum % 11;
    if remainder < 2 { 0 } else { 11 - remainder }
}

/// Validates a CPF's real check digits, accepting both the punctuated
/// (`123.456.789-09`) and bare (`12345678909`) forms. Rejects the
/// all-same-digit numbers (`111.111.111-11`) that satisfy the arithmetic but
/// were never issued.
pub fn is_valid_cpf(value: &str) -> bool {
    if !has_no_stray_characters(value, &['.', '-']) {
        return false;
    }
    let digits: Vec<u32> = only_digits(value)
        .chars()
        .filter_map(|c| c.to_digit(10))
        .collect();
    if digits.len() != 11 || all_same_digit(&digits) {
        return false;
    }
    digits[9] == mod11_check_digit(&digits[..9], false)
        && digits[10] == mod11_check_digit(&digits[..10], false)
}

/// Validates a CNPJ's real check digits, accepting both the punctuated
/// (`12.345.678/0001-95`) and bare (`12345678000195`) forms.
pub fn is_valid_cnpj(value: &str) -> bool {
    if !has_no_stray_characters(value, &['.', '/', '-']) {
        return false;
    }
    let digits: Vec<u32> = only_digits(value)
        .chars()
        .filter_map(|c| c.to_digit(10))
        .collect();
    if digits.len() != 14 || all_same_digit(&digits) {
        return false;
    }
    digits[12] == mod11_check_digit(&digits[..12], true)
        && digits[13] == mod11_check_digit(&digits[..13], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cpf_check_digits_are_accepted_punctuated_and_bare() {
        assert!(is_valid_cpf("111.444.777-35"));
        assert!(is_valid_cpf("11144477735"));
    }

    #[test]
    fn cpf_with_wrong_check_digits_is_rejected() {
        assert!(!is_valid_cpf("111.444.777-36"));
    }

    #[test]
    fn all_same_digit_cpf_is_rejected_despite_passing_the_arithmetic() {
        assert!(!is_valid_cpf("111.111.111-11"));
        assert!(!is_valid_cpf("000.000.000-00"));
    }

    #[test]
    fn cpf_with_wrong_length_is_rejected() {
        assert!(!is_valid_cpf("123"));
        assert!(!is_valid_cpf(""));
    }

    #[test]
    fn valid_cnpj_check_digits_are_accepted_punctuated_and_bare() {
        assert!(is_valid_cnpj("11.222.333/0001-81"));
        assert!(is_valid_cnpj("11222333000181"));
    }

    #[test]
    fn cnpj_with_wrong_check_digits_is_rejected() {
        assert!(!is_valid_cnpj("11.222.333/0001-82"));
    }

    #[test]
    fn all_same_digit_cnpj_is_rejected_despite_passing_the_arithmetic() {
        assert!(!is_valid_cnpj("11.111.111/1111-11"));
    }

    fn texts(values: &[&str]) -> Vec<Value> {
        values.iter().map(|v| Value::Text(v.to_string())).collect()
    }

    #[test]
    fn an_unambiguous_email_column_is_classified_with_high_confidence() {
        let values = texts(&["ana@gmail.com", "bruno@outlook.com", "carla@gmail.com"]);
        let result = classify("email", &values);
        assert_eq!(result.semantic_type, SemanticType::Email);
        assert!(
            result.confidence > 0.9,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn a_misleadingly_named_column_is_classified_by_its_values() {
        // "documento" gives no name hint for any type, but the values are
        // unambiguously valid CPFs.
        let values = texts(&["111.444.777-35", "123.456.789-09"]);
        let result = classify("documento", &values);
        assert_eq!(result.semantic_type, SemanticType::Cpf);
        assert!(result.confidence > 0.5);
    }

    #[test]
    fn a_column_named_like_a_cpf_but_holding_junk_is_not_classified_as_cpf() {
        let values = texts(&["not-a-cpf", "also garbage", "still junk"]);
        let result = classify("cpf", &values);
        assert_ne!(result.semantic_type, SemanticType::Cpf);
    }

    #[test]
    fn an_all_null_column_falls_back_to_the_name_with_low_confidence() {
        let values = vec![Value::Null, Value::Null];
        let result = classify("email_address", &values);
        assert_eq!(result.semantic_type, SemanticType::Email);
        assert!(result.confidence < 0.5);
    }

    #[test]
    fn free_prose_text_is_neither_a_name_nor_any_structured_type() {
        let values = texts(&[
            "the quick brown fox jumps",
            "over the lazy dog near the river",
            "a completely unrelated sentence here",
        ]);
        let result = classify("bio", &values);
        assert_eq!(result.semantic_type, SemanticType::FreeText);
    }

    #[test]
    fn a_pure_numeric_column_is_classified_as_numeric_id() {
        let values = vec![Value::Int(1), Value::Int(2), Value::Int(3)];
        let result = classify("id", &values);
        assert_eq!(result.semantic_type, SemanticType::NumericId);
    }

    #[test]
    fn login_shaped_values_classify_as_username() {
        let values = texts(&["joao.silva92", "ana_costa3", "bruno77lima"]);
        let result = classify("login", &values);
        assert_eq!(result.semantic_type, SemanticType::Username);
    }

    /// A column named `username`/`login`/etc. is the point of the feature:
    /// it must classify `Username` regardless of which handle hint it
    /// uses.
    #[test]
    fn a_column_named_username_classifies_as_username() {
        let values = texts(&["ana.costa1", "bruno_lima2", "carla.rocha3"]);
        let result = classify("username", &values);
        assert_eq!(result.semantic_type, SemanticType::Username);
    }

    /// Machine-readable columns that are shape-conforming (lowercase, a
    /// digit or separator somewhere) but *not* named like a handle must
    /// never classify `Username` — no cardinality threshold can rescue
    /// this, since several of these are near-unique by construction (a
    /// slug or file name derived from a per-row title, a SKU that never
    /// repeats) and one is a lookup table with one row per value (equally
    /// "unique" over its own tiny sample). The column's name is the only
    /// signal that actually separates these from a real login.
    #[test]
    fn unnamed_machine_readable_columns_are_never_classified_as_username() {
        type NameAndValueMaker = (&'static str, fn(usize) -> String);
        let unique_cases: &[NameAndValueMaker] = &[
            ("slug", |i| format!("meu-post-numero-{i}")),
            ("sku", |i| format!("widget-pro-{i}")),
            ("file_name", |i| format!("relatorio_2023_{i:03}.pdf")),
            ("event_name", |i| format!("checkout.step{i}.completed")),
        ];
        for (name, make_value) in unique_cases {
            let values: Vec<Value> = (0..20).map(|i| Value::Text(make_value(i))).collect();
            let result = classify(name, &values);
            assert_ne!(
                result.semantic_type,
                SemanticType::Username,
                "`{name}` with unique-by-construction values must not classify as Username"
            );
        }

        // A lookup table: one row per enum value, so every sampled value is
        // pairwise distinct — cardinality alone would have called this a
        // login, same as the columns above.
        let lookup_cases: &[(&str, &[&str])] = &[
            ("locale", &["pt-br", "en-us", "es-mx"]),
            ("status", &["in_progress", "not_started", "in_review"]),
        ];
        for (name, values) in lookup_cases {
            let column = texts(values);
            let result = classify(name, &column);
            assert_ne!(
                result.semantic_type,
                SemanticType::Username,
                "`{name}` lookup table must not classify as Username"
            );
        }
    }

    /// The flip side: a column *named* like a handle but holding values
    /// that overwhelmingly fail the shape check must still fall through —
    /// the name makes `Username` eligible, it does not make it win
    /// unconditionally. [`looks_like_username`] is a confirming check, not
    /// a formality.
    #[test]
    fn a_handle_named_column_with_unshaped_values_does_not_classify_as_username() {
        let values = texts(&["not a handle", "also not one", "still not one"]);
        let result = classify("login", &values);
        assert_ne!(result.semantic_type, SemanticType::Username);
    }

    /// An unnamed handle-shaped, high-cardinality column is a known,
    /// accepted miss: name evidence is necessary, so shape and cardinality
    /// alone cannot carry it. Distinguishing this from the cases above is
    /// the whole point of the design — recorded here so a future change
    /// that makes this pass is a deliberate decision, not a silent
    /// regression back to shape-only matching.
    #[test]
    fn an_unnamed_login_shaped_column_is_a_known_miss() {
        let values = texts(&[
            "ana.costa1",
            "bruno_lima2",
            "carla.rocha3",
            "davi_santos4",
            "elisa.souza5",
        ]);
        let result = classify("identifier", &values);
        assert_ne!(result.semantic_type, SemanticType::Username);
    }

    /// A column named like a login but holding nothing but nulls must
    /// never classify as `Username`: there is no value to confirm the
    /// shape against, and [`classify_by_name_only`] excludes `Username`
    /// entirely for exactly this reason. Regression coverage for a real
    /// defect: before this exclusion existed, an all-null `last_login` or
    /// `nickname` column of a non-`Text` declared type classified
    /// `Username` from the name alone, then failed at strategy resolution
    /// with an incompatible-strategy error — a dataset `main` anonymizes
    /// successfully, this branch did not.
    #[test]
    fn an_all_null_column_named_like_a_login_does_not_classify_as_username() {
        let values = vec![Value::Null, Value::Null, Value::Null];
        let result = classify("last_login", &values);
        assert_ne!(result.semantic_type, SemanticType::Username);
        assert_eq!(result.semantic_type, SemanticType::Unknown);
    }

    #[test]
    fn full_names_are_recognised_from_values_alone() {
        let values = texts(&["José da Silva", "Ana Paula Souza", "Ñoño Ñandú"]);
        let result = classify("campo_x", &values);
        assert_eq!(result.semantic_type, SemanticType::FullName);
    }

    /// Exhaustiveness guard for [`SemanticType::ALL`]: this match has no
    /// wildcard arm, so adding a variant to the enum without adding it here
    /// — and to `ALL` itself — fails to compile.
    fn assert_every_variant_is_covered(ty: SemanticType) {
        match ty {
            SemanticType::Email
            | SemanticType::Uuid
            | SemanticType::Cpf
            | SemanticType::Cnpj
            | SemanticType::Phone
            | SemanticType::Cep
            | SemanticType::FullName
            | SemanticType::Date
            | SemanticType::Username
            | SemanticType::NumericId
            | SemanticType::FreeText
            | SemanticType::Unknown => {}
        }
    }

    #[test]
    fn all_lists_every_semantic_type_variant_exactly_once() {
        let seen: std::collections::BTreeSet<SemanticType> =
            SemanticType::ALL.iter().copied().collect();
        for ty in SemanticType::ALL {
            assert_every_variant_is_covered(*ty);
        }
        assert_eq!(seen.len(), SemanticType::ALL.len(), "ALL names a duplicate");
        assert_eq!(
            seen.len(),
            12,
            "update ALL and assert_every_variant_is_covered together"
        );
    }

    #[test]
    fn a_hyphenated_uuid_column_is_classified_as_uuid() {
        let values = texts(&[
            "550e8400-e29b-41d4-a716-446655440000",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
        ]);
        let result = classify("entity_uuid", &values);
        assert_eq!(result.semantic_type, SemanticType::Uuid);
    }

    #[test]
    fn a_bare_uuid_column_is_classified_as_uuid() {
        let values = texts(&["550e8400e29b41d4a716446655440000"]);
        let result = classify("ref", &values);
        assert_eq!(result.semantic_type, SemanticType::Uuid);
    }

    /// One UUID-shaped value among many unrelated free-text values must not
    /// be enough to classify the whole column `Uuid` — a majority of the
    /// sampled values has to actually be UUID-shaped, since every value in
    /// a `Uuid` column mints through the dataset-wide identity space.
    #[test]
    fn a_single_uuid_among_mostly_free_text_values_does_not_classify_as_uuid() {
        let mut raw = vec!["550e8400-e29b-41d4-a716-446655440000".to_owned()];
        raw.extend((0..10).map(|i| format!("not a uuid at all {i}")));
        let values: Vec<Value> = raw.into_iter().map(Value::Text).collect();
        let result = classify("entity_uuid", &values);
        assert_ne!(result.semantic_type, SemanticType::Uuid);
    }

    /// A CPF stored as `BIGINT` must classify as `Cpf`: the check-digit
    /// test [`classify_int_document_shape`] runs against each integer's
    /// decimal form makes a random sequential id an unlikely false
    /// positive.
    #[test]
    fn a_bigint_column_of_real_cpfs_classifies_as_cpf() {
        let values = vec![
            Value::Int(11_144_477_735),
            Value::Int(12_345_678_909),
            Value::Int(52_998_224_725),
        ];
        let result = classify("documento", &values);
        assert_eq!(result.semantic_type, SemanticType::Cpf);
    }

    /// A plain sequential integer id, too short to accidentally match any
    /// document pattern, must stay `NumericId` rather than a false
    /// positive.
    #[test]
    fn a_bigint_column_of_sequential_ids_stays_numeric_id() {
        let values: Vec<Value> = (1..=20i64).map(Value::Int).collect();
        let result = classify("id", &values);
        assert_eq!(result.semantic_type, SemanticType::NumericId);
    }

    /// A sequential id must stay `NumericId` at every width from 4 to 14 —
    /// width alone must never classify it as `Cep` or `Phone`, since a
    /// sequential id at any of those widths has no check digit to
    /// coincidentally satisfy.
    #[test]
    fn sequential_ids_stay_numeric_id_at_every_width_from_4_to_14() {
        for width in 4..=14u32 {
            let base = 10i64.pow(width - 1);
            let values: Vec<Value> = (base..base + 20).map(Value::Int).collect();
            let result = classify("id", &values);
            assert_eq!(
                result.semantic_type,
                SemanticType::NumericId,
                "width {width} misclassified as {:?}",
                result.semantic_type
            );
        }
    }

    #[test]
    fn a_bare_format_cpf_classifies_as_cpf_not_phone() {
        let values = texts(&["11144477735"]);
        let result = classify("cpf", &values);
        assert_eq!(result.semantic_type, SemanticType::Cpf);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn a_bare_format_cnpj_classifies_as_cnpj_not_phone() {
        let values = texts(&["11222333000181"]);
        let result = classify("cnpj", &values);
        assert_eq!(result.semantic_type, SemanticType::Cnpj);
        assert_eq!(result.confidence, 1.0);
    }

    #[test]
    fn a_cpf_framed_by_junk_characters_is_rejected() {
        assert!(!is_valid_cpf("x111.444.777-35x"));
    }

    #[test]
    fn a_cnpj_framed_by_junk_characters_is_rejected() {
        assert!(!is_valid_cnpj("abc11222333000181"));
    }
}
