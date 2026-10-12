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

/// What a column means, as far as this build can tell. Enum dispatch: adding a
/// detector is a variant plus an arm, never a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticType {
    Email,
    Cpf,
    Cnpj,
    Phone,
    Cep,
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
        SemanticType::Cpf,
        SemanticType::Cnpj,
        SemanticType::Phone,
        SemanticType::Cep,
        SemanticType::FullName,
        SemanticType::Date,
        SemanticType::NumericId,
        SemanticType::FreeText,
        SemanticType::Unknown,
    ];

    /// The patterns scored against sampled values. Earlier entries win both
    /// the name-only fallback and a tie in value-match rate: [`classify_text`]
    /// keeps the first type it meets at the current best rate rather than
    /// the last, so this slice's order is the real priority, not a cosmetic
    /// grouping.
    const STRUCTURED_TEXT_TYPES: &'static [SemanticType] = &[
        SemanticType::Email,
        SemanticType::Cpf,
        SemanticType::Cnpj,
        SemanticType::Phone,
        SemanticType::Cep,
        SemanticType::FullName,
        SemanticType::Date,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SemanticType::Email => "email",
            SemanticType::Cpf => "cpf",
            SemanticType::Cnpj => "cnpj",
            SemanticType::Phone => "phone",
            SemanticType::Cep => "cep",
            SemanticType::FullName => "full_name",
            SemanticType::Date => "date",
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
        )
    }

    fn name_hints(self) -> &'static [&'static str] {
        match self {
            SemanticType::Email => &["email", "e-mail", "e_mail", "correio"],
            SemanticType::Cpf => &["cpf"],
            SemanticType::Cnpj => &["cnpj"],
            SemanticType::Phone => &["phone", "telefone", "celular", "fone", "tel"],
            SemanticType::Cep => &["cep", "zipcode", "zip", "postal"],
            SemanticType::FullName => &["name", "nome", "fullname"],
            SemanticType::Date => &["date", "data", "dt", "nascimento", "birthdate"],
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
            SemanticType::Cpf => is_valid_cpf(text),
            SemanticType::Cnpj => is_valid_cnpj(text),
            SemanticType::Phone => PHONE_RE.is_match(text),
            SemanticType::Cep => CEP_RE.is_match(text),
            SemanticType::Date => DATE_RE.is_match(text),
            SemanticType::FullName => looks_like_full_name(text),
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

fn classify_text(name: &str, texts: &[&str]) -> Classification {
    // `max_by` returns the *last* maximum on a tie, so the comparator orders
    // by rate first and, on a tie, treats the earlier candidate as the
    // greater one: the one already chosen stays chosen.
    let scored = SemanticType::STRUCTURED_TEXT_TYPES
        .iter()
        .map(|&ty| (ty, text_match_rate(ty, texts)))
        .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));

    match scored {
        Some((semantic_type, rate)) if rate > 0.0 => Classification {
            semantic_type,
            confidence: boost(rate, name_hints_match(semantic_type, name)),
        },
        _ => Classification {
            semantic_type: SemanticType::FreeText,
            confidence: FREE_TEXT_CONFIDENCE,
        },
    }
}

fn classify_by_name_only(name: &str) -> Classification {
    for &semantic_type in SemanticType::STRUCTURED_TEXT_TYPES {
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
            | SemanticType::Cpf
            | SemanticType::Cnpj
            | SemanticType::Phone
            | SemanticType::Cep
            | SemanticType::FullName
            | SemanticType::Date
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
            10,
            "update ALL and assert_every_variant_is_covered together"
        );
    }

    /// M4 regression: a bare-format CPF used to lose the tie against `Phone`
    /// because `max_by` kept the last maximum rather than the first.
    #[test]
    fn a_bare_format_cpf_classifies_as_cpf_not_phone() {
        let values = texts(&["11144477735"]);
        let result = classify("cpf", &values);
        assert_eq!(result.semantic_type, SemanticType::Cpf);
        assert_eq!(result.confidence, 1.0);
    }

    /// M4 regression: the CNPJ counterpart of the above.
    #[test]
    fn a_bare_format_cnpj_classifies_as_cnpj_not_phone() {
        let values = texts(&["11222333000181"]);
        let result = classify("cnpj", &values);
        assert_eq!(result.semantic_type, SemanticType::Cnpj);
        assert_eq!(result.confidence, 1.0);
    }

    /// M6 regression: digit-stripping alone validated a CPF framed by
    /// arbitrary junk, since every character outside the digits was simply
    /// discarded before the check digits were verified.
    #[test]
    fn a_cpf_framed_by_junk_characters_is_rejected() {
        assert!(!is_valid_cpf("x111.444.777-35x"));
    }

    /// M6 regression: the CNPJ counterpart of the above.
    #[test]
    fn a_cnpj_framed_by_junk_characters_is_rejected() {
        assert!(!is_valid_cnpj("abc11222333000181"));
    }
}
