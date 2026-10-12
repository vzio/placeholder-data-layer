pub mod ledger;

use std::collections::BTreeMap;

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::classify::SemanticType;
use crate::dataset::{ColumnSpec, KeyValue, Value};
use crate::error::Error;
use crate::transform::ledger::{ClassId, Ledger};

/// A small bundled pt-BR corpus [`resample_name`] draws from. Not real
/// people: a fixed, modest list whose only job is to be plausible.
const FIRST_NAMES: &[&str] = &[
    "Ana", "Bruno", "Carla", "Davi", "Elisa", "Fabio", "Giovana", "Hugo", "Iris", "Joao", "Karla",
    "Lucas", "Marina", "Nina", "Otavio", "Paula", "Quintino", "Rafaela", "Sofia", "Tiago",
];

const SURNAMES: &[&str] = &[
    "Silva",
    "Souza",
    "Lima",
    "Pereira",
    "Costa",
    "Alves",
    "Rocha",
    "Martins",
    "Fernandes",
    "Ribeiro",
    "Nunes",
    "Teixeira",
    "Carvalho",
    "Barros",
    "Correia",
    "Moreira",
    "Castro",
    "Pinto",
    "Araujo",
    "Cardoso",
];

/// Domains [`pseudonymize_email`] falls back to when the source has no
/// domain of its own to keep (an address with no `@` at all).
const FALLBACK_EMAIL_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];

/// Every transform is local to the value and preserves the part that carries
/// the distribution, so fidelity holds by construction rather than by
/// resampling. Enum dispatch: no `dyn Strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Leave the value untouched.
    Keep,
    /// Length-preserving redaction. The conservative default.
    Redact,
    /// New local part, domain kept verbatim.
    PseudonymizeLocalPart,
    /// Area code and prefix kept, remainder randomized.
    PreservePrefix,
    /// Regenerated with valid check digits.
    RegenerateDocument,
    /// Resampled from the bundled pt-BR corpus; token count and casing kept.
    ResampleName,
    /// Moved within a bounded window.
    Jitter,
    /// Minted through the ledger.
    MintKey,
}

impl Strategy {
    /// The strategy a column falls back to when [`resolve`] finds no
    /// override: a function of the column's semantic type alone. Key
    /// columns never reach this — [`resolve`] dispatches those to
    /// [`Strategy::MintKey`] via [`crate::dataset::ColumnSpec::is_key`]
    /// before semantic type is even consulted, since a numeric column that
    /// happens to be a primary or foreign key must mint through the ledger,
    /// not jitter like an ordinary measurement.
    pub fn for_semantic_type(semantic_type: SemanticType) -> Strategy {
        match semantic_type {
            SemanticType::Email => Strategy::PseudonymizeLocalPart,
            SemanticType::Phone | SemanticType::Cep => Strategy::PreservePrefix,
            SemanticType::Cpf | SemanticType::Cnpj => Strategy::RegenerateDocument,
            SemanticType::FullName => Strategy::ResampleName,
            SemanticType::Date | SemanticType::NumericId => Strategy::Jitter,
            SemanticType::FreeText | SemanticType::Unknown => Strategy::Redact,
        }
    }

    /// Applies this strategy to one value of `column`, used only to name
    /// `column` in [`Error::IncompatibleStrategy`] should this strategy and
    /// `value`'s shape turn out incompatible (a `MintKey` override on a
    /// non-key column, say). `rng` must be derived per value, keyed on
    /// whatever identifies the value for this strategy's consistency
    /// requirements — [`crate::context::RunContext::rng_for_row_position`]
    /// for every strategy here except [`Strategy::MintKey`], which instead
    /// looks up an already-minted value and draws no randomness of its own.
    pub fn apply(
        self,
        column: &str,
        value: &Value,
        strength: Strength,
        ledger: &Ledger,
        class: Option<ClassId>,
        rng: &mut ChaCha8Rng,
    ) -> Result<Value, Error> {
        if value.is_null() {
            return Ok(Value::Null);
        }

        if self == Strategy::MintKey {
            return mint_key(column, value, ledger, class);
        }

        match (self, value) {
            (Strategy::Keep, _) => Ok(value.clone()),
            (Strategy::Redact, Value::Text(text)) => Ok(Value::Text(redact(text, strength))),
            (Strategy::Redact, other) => Ok(other.clone()),
            (Strategy::PseudonymizeLocalPart, Value::Text(text)) => Ok(Value::Text(
                pseudonymize_email(&sanitize(text), strength, rng),
            )),
            (Strategy::PreservePrefix, Value::Text(text)) => {
                Ok(Value::Text(preserve_prefix(&sanitize(text), strength, rng)))
            }
            (Strategy::RegenerateDocument, Value::Text(text)) => Ok(Value::Text(
                regenerate_document(&sanitize(text), strength, rng),
            )),
            (Strategy::ResampleName, Value::Text(text)) => {
                Ok(Value::Text(resample_name(&sanitize(text), strength, rng)))
            }
            (Strategy::Jitter, Value::Text(text)) => {
                Ok(Value::Text(jitter_date(&sanitize(text), strength, rng)))
            }
            (Strategy::Jitter, Value::Int(_) | Value::Float(_)) => {
                Ok(jitter_numeric(value, strength, rng))
            }
            _ => Err(Error::IncompatibleStrategy {
                column: column.to_owned(),
                strategy: self,
            }),
        }
    }
}

/// Escalation ladder for the compliance retry loop. Termination is proven by
/// the type: the ladder is finite and `next` eventually returns `None`, so no
/// `max_retries` counter is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Default,
    Aggressive,
    Maximum,
}

impl Strength {
    pub fn next(self) -> Option<Strength> {
        match self {
            Strength::Default => Some(Strength::Aggressive),
            Strength::Aggressive => Some(Strength::Maximum),
            Strength::Maximum => None,
        }
    }
}

/// Resolves the strategy `qualified` runs under: an explicit override when
/// `overrides` names one, otherwise [`Strategy::MintKey`] for a key column
/// or [`Strategy::for_semantic_type`] for any other. Rejects a combination
/// that cannot work: `MintKey` requires a ledger class and therefore a key
/// column, and a key column requires `MintKey` — overriding either side
/// away from that pairing would either crash on lookup or quietly break
/// referential integrity, so both are refused up front instead.
pub(crate) fn resolve(
    overrides: &BTreeMap<String, Strategy>,
    qualified: &str,
    spec: &ColumnSpec,
    semantic_type: SemanticType,
) -> Result<Strategy, Error> {
    let is_key = spec.is_key();
    let strategy = overrides.get(qualified).copied().unwrap_or_else(|| {
        if is_key {
            Strategy::MintKey
        } else {
            Strategy::for_semantic_type(semantic_type)
        }
    });

    if (strategy == Strategy::MintKey) != is_key {
        return Err(Error::IncompatibleStrategy {
            column: qualified.to_owned(),
            strategy,
        });
    }
    Ok(strategy)
}

fn mint_key(
    column: &str,
    value: &Value,
    ledger: &Ledger,
    class: Option<ClassId>,
) -> Result<Value, Error> {
    fn incompatible(column: &str) -> Error {
        Error::IncompatibleStrategy {
            column: column.to_owned(),
            strategy: Strategy::MintKey,
        }
    }

    let class = class.ok_or_else(|| incompatible(column))?;
    let key = KeyValue::from_value(value).ok_or_else(|| incompatible(column))?;
    ledger
        .lookup(class, &key)
        .cloned()
        .map(Value::from)
        .ok_or_else(|| incompatible(column))
}

/// Strips the two generic text defects that would otherwise survive any
/// strategy that copies characters from the source verbatim: a stray
/// encoding artifact, and whitespace padding or runs. Every text-producing
/// strategy below starts from this, so a malformed *source* value never
/// makes its malformed *shape* (as opposed to its content) the thing that
/// gets carried into the anonymized output — that shape is exactly what
/// [`crate::anomaly::reinject`] reproduces separately, at the observed
/// rate, never by letting the original defect fall through.
fn sanitize(text: &str) -> String {
    let mut cleaned = text.replace('\u{FFFD}', "");
    for marker in crate::anomaly::MOJIBAKE_MARKERS {
        cleaned = cleaned.replace(marker, "");
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn redact(text: &str, strength: Strength) -> String {
    let sanitized = sanitize(text);
    match strength {
        Strength::Default => sanitized.chars().map(mask_char).collect(),
        Strength::Aggressive => "REDACTED".to_owned(),
        Strength::Maximum => String::new(),
    }
}

fn mask_char(c: char) -> char {
    if c.is_ascii_digit() {
        '0'
    } else if c.is_uppercase() {
        'X'
    } else if c.is_lowercase() {
        'x'
    } else {
        c
    }
}

fn pseudonymize_email(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let domain = text
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_owned())
        .filter(|domain| !domain.is_empty())
        .unwrap_or_else(|| {
            FALLBACK_EMAIL_DOMAINS[rng.random_range(0..FALLBACK_EMAIL_DOMAINS.len())].to_owned()
        });
    let local = random_token(local_part_len(strength), rng);
    format!("{local}@{domain}")
}

fn local_part_len(strength: Strength) -> usize {
    match strength {
        Strength::Default => 10,
        Strength::Aggressive => 6,
        Strength::Maximum => 4,
    }
}

fn random_token(len: usize, rng: &mut ChaCha8Rng) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..len)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Keeps the leading digits that carry the shared part of a phone number or
/// CEP (the DDD area code, the CEP region) and randomizes the rest,
/// digit-by-digit, leaving every other character (punctuation, and any
/// stray non-digit the source happened to contain) exactly where it was. A
/// value with no digits at all has no phone or CEP shape to preserve a
/// prefix of — it is free text wearing this column's name, and every
/// character of it would otherwise survive verbatim — so it falls through
/// to [`redact`] instead.
fn preserve_prefix(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let digit_count = text.chars().filter(char::is_ascii_digit).count();
    if digit_count == 0 {
        return redact(text, strength);
    }
    let prefix_len = prefix_length(digit_count, strength);
    let mut seen_digits = 0usize;
    text.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                let keep = seen_digits < prefix_len;
                seen_digits += 1;
                if keep { c } else { random_digit(rng) }
            } else {
                c
            }
        })
        .collect()
}

fn prefix_length(digit_count: usize, strength: Strength) -> usize {
    let base = match digit_count {
        8 => 5,       // CEP: the regional prefix.
        10 | 11 => 2, // Phone: the DDD area code.
        _ => digit_count / 3,
    };
    match strength {
        Strength::Default => base,
        Strength::Aggressive => base / 2,
        Strength::Maximum => 0,
    }
}

fn random_digit(rng: &mut ChaCha8Rng) -> char {
    char::from(b'0' + rng.random_range(0..10u8))
}

/// Regenerates a CPF (11 digits) or CNPJ (14 digits) from fresh random
/// digits with real check digits, reusing
/// [`crate::classify::mod11_check_digit`] rather than reimplementing the
/// arithmetic. Always well-formed by construction, independent of whether
/// the source was: there is no partial-credit way to "preserve" a malformed
/// document number, so this never tries to. A digit count that is neither
/// has no document shape to regenerate from, so it falls through to
/// [`redact`] rather than surviving as the raw source text.
fn regenerate_document(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let digit_count = text.chars().filter(char::is_ascii_digit).count();
    let punctuated =
        strength != Strength::Maximum && text.chars().any(|c| matches!(c, '.' | '-' | '/'));
    match digit_count {
        11 => format_document(&cpf_digits(rng), punctuated, false),
        14 => format_document(&cnpj_digits(rng), punctuated, true),
        _ => redact(text, strength),
    }
}

fn cpf_digits(rng: &mut ChaCha8Rng) -> Vec<u32> {
    loop {
        let base = random_digits(9, rng);
        if all_same(&base) {
            continue;
        }
        return with_check_digits(base, false);
    }
}

fn cnpj_digits(rng: &mut ChaCha8Rng) -> Vec<u32> {
    loop {
        let base = random_digits(12, rng);
        if all_same(&base) {
            continue;
        }
        return with_check_digits(base, true);
    }
}

fn with_check_digits(base: Vec<u32>, wrap: bool) -> Vec<u32> {
    let mut digits = base;
    let first = crate::classify::mod11_check_digit(&digits, wrap);
    digits.push(first);
    let second = crate::classify::mod11_check_digit(&digits, wrap);
    digits.push(second);
    digits
}

fn random_digits(n: usize, rng: &mut ChaCha8Rng) -> Vec<u32> {
    (0..n).map(|_| rng.random_range(0..10u32)).collect()
}

fn all_same(digits: &[u32]) -> bool {
    digits
        .first()
        .is_some_and(|&first| digits.iter().all(|&d| d == first))
}

fn format_document(digits: &[u32], punctuated: bool, is_cnpj: bool) -> String {
    let plain: String = digits.iter().map(|d| char::from(b'0' + *d as u8)).collect();
    if !punctuated {
        return plain;
    }
    if is_cnpj {
        format!(
            "{}.{}.{}/{}-{}",
            &plain[0..2],
            &plain[2..5],
            &plain[5..8],
            &plain[8..12],
            &plain[12..14]
        )
    } else {
        format!(
            "{}.{}.{}-{}",
            &plain[0..3],
            &plain[3..6],
            &plain[6..9],
            &plain[9..11]
        )
    }
}

/// Resamples a full name from [`FIRST_NAMES`] and [`SURNAMES`], keeping the
/// source's token count and emitting standard Title Case regardless of the
/// source's own casing: a malformed source value's casing is exactly the
/// defect [`crate::anomaly::reinject`] reproduces separately, so carrying it
/// through here would double it up rather than reproduce it once.
fn resample_name(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let token_count = text.split_whitespace().count().max(1);
    let (first_names, surnames) = name_pool(strength);
    (0..token_count)
        .map(|index| {
            let pool = if index == 0 { first_names } else { surnames };
            pool[rng.random_range(0..pool.len())]
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stronger strength draws from a smaller slice of the corpus, collapsing
/// more source names onto the same handful of resampled ones — more
/// generalization, at the cost of looking more repetitive across rows.
fn name_pool(strength: Strength) -> (&'static [&'static str], &'static [&'static str]) {
    match strength {
        Strength::Default => (FIRST_NAMES, SURNAMES),
        Strength::Aggressive => (&FIRST_NAMES[..8], &SURNAMES[..8]),
        Strength::Maximum => (&FIRST_NAMES[..3], &SURNAMES[..3]),
    }
}

fn jitter_numeric(value: &Value, strength: Strength, rng: &mut ChaCha8Rng) -> Value {
    match value {
        Value::Int(n) => Value::Int(jitter_int(*n, strength, rng)),
        Value::Float(f) => Value::Float(jitter_float(*f, strength, rng)),
        other => other.clone(),
    }
}

/// Jitter window as a fraction of the value's own magnitude: this transform
/// never sees the rest of the column, so the window can only scale with the
/// value itself, not with the column's observed spread.
fn jitter_window(strength: Strength) -> f64 {
    match strength {
        Strength::Default => 0.05,
        Strength::Aggressive => 0.15,
        Strength::Maximum => 0.35,
    }
}

fn jitter_int(n: i64, strength: Strength, rng: &mut ChaCha8Rng) -> i64 {
    let magnitude = ((n.unsigned_abs() as f64) * jitter_window(strength)).max(1.0) as i64;
    n.saturating_add(rng.random_range(-magnitude..=magnitude))
}

fn jitter_float(f: f64, strength: Strength, rng: &mut ChaCha8Rng) -> f64 {
    let magnitude = (f.abs() * jitter_window(strength)).max(1e-6);
    f + rng.random_range(-magnitude..=magnitude)
}

fn jitter_days(strength: Strength) -> i64 {
    match strength {
        Strength::Default => 30,
        Strength::Aggressive => 180,
        Strength::Maximum => 730,
    }
}

/// Jitters a date within a bounded day window, reusing
/// [`crate::anomaly::parse_date_fields`] and
/// [`crate::anomaly::days_in_month`] rather than a second calendar parser. A
/// value that does not parse as a real calendar date has no "nearby" date to
/// move it to — it is already the kind of defect
/// [`crate::anomaly::AnomalyKind::ImpossibleDate`] exists to reproduce — so
/// it falls through to [`redact`] rather than surviving as the raw source
/// text.
fn jitter_date(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let Some((year, month, day)) = crate::anomaly::parse_date_fields(text) else {
        return redact(text, strength);
    };
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > crate::anomaly::days_in_month(year, month)
    {
        return redact(text, strength);
    }

    let window = jitter_days(strength);
    let delta = rng.random_range(-window..=window);
    let (new_year, new_month, new_day) = add_days(year, month, day, delta);
    format_date_like(text, new_year, new_month, new_day)
}

fn add_days(mut year: u32, mut month: u32, mut day: u32, delta: i64) -> (u32, u32, u32) {
    let mut remaining = delta;
    while remaining > 0 {
        day += 1;
        if day > crate::anomaly::days_in_month(year, month) {
            day = 1;
            month += 1;
            if month > 12 {
                month = 1;
                year += 1;
            }
        }
        remaining -= 1;
    }
    while remaining < 0 {
        if day > 1 {
            day -= 1;
        } else {
            if month == 1 {
                month = 12;
                year = year.saturating_sub(1);
            } else {
                month -= 1;
            }
            day = crate::anomaly::days_in_month(year, month);
        }
        remaining += 1;
    }
    (year, month, day)
}

fn format_date_like(original: &str, year: u32, month: u32, day: u32) -> String {
    let bytes = original.as_bytes();
    if bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' {
        return format!("{year:04}-{month:02}-{day:02}");
    }
    if bytes.len() == 10 {
        let separator = bytes[2] as char;
        return format!("{day:02}{separator}{month:02}{separator}{year:04}");
    }
    original.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify;
    use crate::dataset::{ColumnType, ForeignKey};

    fn rng(label: &str) -> ChaCha8Rng {
        crate::rng::derive(1, label.as_bytes())
    }

    fn key_spec(name: &str, pk: bool, fk: Option<ForeignKey>) -> ColumnSpec {
        ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk,
            fk,
        }
    }

    fn plain_spec(name: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
            nullable: true,
            pk: false,
            fk: None,
        }
    }

    #[test]
    fn strength_ladder_terminates_after_maximum() {
        assert_eq!(Strength::Default.next(), Some(Strength::Aggressive));
        assert_eq!(Strength::Aggressive.next(), Some(Strength::Maximum));
        assert_eq!(Strength::Maximum.next(), None);
    }

    #[test]
    fn resolve_sends_a_key_column_to_mint_key_regardless_of_semantic_type() {
        let overrides = BTreeMap::new();
        let spec = key_spec("id", true, None);
        let strategy =
            resolve(&overrides, "t.id", &spec, SemanticType::FreeText).expect("resolves");
        assert_eq!(strategy, Strategy::MintKey);
    }

    #[test]
    fn resolve_honours_an_override_on_a_non_key_column() {
        let mut overrides = BTreeMap::new();
        overrides.insert("t.notes".to_owned(), Strategy::Redact);
        let spec = plain_spec("notes");
        let strategy =
            resolve(&overrides, "t.notes", &spec, SemanticType::FreeText).expect("resolves");
        assert_eq!(strategy, Strategy::Redact);
    }

    #[test]
    fn resolve_rejects_mint_key_overriding_a_non_key_column() {
        let mut overrides = BTreeMap::new();
        overrides.insert("t.notes".to_owned(), Strategy::MintKey);
        let spec = plain_spec("notes");
        let error =
            resolve(&overrides, "t.notes", &spec, SemanticType::FreeText).expect_err("rejected");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    #[test]
    fn resolve_rejects_a_non_mint_key_override_on_a_key_column() {
        let mut overrides = BTreeMap::new();
        overrides.insert("t.id".to_owned(), Strategy::Redact);
        let spec = key_spec("id", true, None);
        let error =
            resolve(&overrides, "t.id", &spec, SemanticType::NumericId).expect_err("rejected");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    #[test]
    fn email_keeps_the_domain_verbatim_and_changes_the_local_part() {
        let mut r = rng("email");
        let out = pseudonymize_email("ana@gmail.com", Strength::Default, &mut r);
        let (local, domain) = out.split_once('@').expect("well-formed email");
        assert_eq!(domain, "gmail.com");
        assert_ne!(local, "ana");
    }

    #[test]
    fn email_with_no_at_sign_still_produces_a_well_formed_address() {
        let mut r = rng("email-malformed");
        let out = pseudonymize_email("not-an-email", Strength::Default, &mut r);
        assert!(
            classify::SemanticType::Email.matches_text(&out),
            "`{out}` is not email-shaped"
        );
    }

    #[test]
    fn email_with_a_stray_at_sign_still_keeps_the_trailing_domain() {
        let mut r = rng("email-stray-at");
        let out = pseudonymize_email("also@@broken.com", Strength::Default, &mut r);
        assert!(
            out.ends_with("@broken.com"),
            "`{out}` must keep the trailing domain"
        );
        assert!(classify::SemanticType::Email.matches_text(&out));
    }

    #[test]
    fn preserve_prefix_keeps_the_ddd_for_a_phone_number() {
        let mut r = rng("phone");
        let out = preserve_prefix("(11) 98765-4321", Strength::Default, &mut r);
        assert!(out.starts_with("(11)"), "`{out}` must keep the DDD");
        assert_eq!(out.len(), "(11) 98765-4321".len());
    }

    /// C2 regression: a phone-column value with no digits at all used to
    /// survive `preserve_prefix` byte-for-byte, since there was nothing for
    /// it to randomize.
    #[test]
    fn preserve_prefix_redacts_a_value_with_no_digits_at_all() {
        let mut r = rng("phone-no-digits");
        let out = preserve_prefix("NAO TEM TELEFONE", Strength::Default, &mut r);
        assert_ne!(out, "NAO TEM TELEFONE");
    }

    #[test]
    fn preserve_prefix_at_maximum_strength_keeps_no_digits() {
        let mut r = rng("cep-max");
        let out = preserve_prefix("01310-100", Strength::Maximum, &mut r);
        assert_ne!(out, "01310-100");
        assert_eq!(out.len(), "01310-100".len());
    }

    #[test]
    fn regenerated_cpf_always_has_valid_check_digits() {
        for seed in 0..20u64 {
            let mut r = crate::rng::derive(seed, b"cpf");
            let out = regenerate_document("111.444.777-35", Strength::Default, &mut r);
            assert!(classify::is_valid_cpf(&out), "`{out}` is not a valid CPF");
        }
    }

    #[test]
    fn regenerated_cnpj_always_has_valid_check_digits() {
        for seed in 0..20u64 {
            let mut r = crate::rng::derive(seed, b"cnpj");
            let out = regenerate_document("11.222.333/0001-81", Strength::Default, &mut r);
            assert!(classify::is_valid_cnpj(&out), "`{out}` is not a valid CNPJ");
        }
    }

    #[test]
    fn regenerated_document_at_maximum_strength_drops_punctuation() {
        let mut r = rng("cpf-max");
        let out = regenerate_document("111.444.777-35", Strength::Maximum, &mut r);
        assert!(out.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(out.len(), 11);
    }

    /// C2 regression: a cpf-column value whose digit count is neither 11 nor
    /// 14 used to survive `regenerate_document` byte-for-byte.
    #[test]
    fn regenerate_document_redacts_a_digit_count_that_matches_no_document_shape() {
        let mut r = rng("document-wrong-digit-count");
        let out = regenerate_document("123.456.789-0", Strength::Default, &mut r);
        assert_ne!(out, "123.456.789-0");
    }

    #[test]
    fn resample_name_preserves_token_count_and_uses_title_case() {
        let mut r = rng("name");
        let out = resample_name("ALL CAPS NAME", Strength::Default, &mut r);
        let tokens: Vec<&str> = out.split_whitespace().collect();
        assert_eq!(tokens.len(), 3);
        for token in tokens {
            let mut chars = token.chars();
            assert!(chars.next().is_some_and(|c| c.is_uppercase()));
            assert!(chars.all(|c| c.is_lowercase()));
        }
    }

    #[test]
    fn resample_name_single_token_stays_single_token() {
        let mut r = rng("single-name");
        let out = resample_name("Madonna", Strength::Default, &mut r);
        assert_eq!(out.split_whitespace().count(), 1);
    }

    #[test]
    fn jitter_int_stays_within_the_strength_window() {
        let mut r = rng("jitter-int");
        let out = jitter_int(1000, Strength::Default, &mut r);
        assert!(
            (out - 1000).abs() <= 50,
            "jitter of {out} exceeded the default window"
        );
    }

    #[test]
    fn jitter_date_produces_a_valid_nearby_date() {
        let mut r = rng("jitter-date");
        let out = jitter_date("2024-01-15", Strength::Default, &mut r);
        let (year, month, day) = crate::anomaly::parse_date_fields(&out).expect("still parses");
        assert!(day >= 1 && day <= crate::anomaly::days_in_month(year, month));
        assert!((1..=12).contains(&month));
    }

    #[test]
    fn jitter_date_preserves_the_slash_dmy_format() {
        let mut r = rng("jitter-date-dmy");
        let out = jitter_date("31/12/2023", Strength::Default, &mut r);
        assert_eq!(out.len(), 10);
        assert_eq!(out.as_bytes()[2], b'/');
        assert_eq!(out.as_bytes()[5], b'/');
    }

    #[test]
    fn jitter_date_redacts_rather_than_leaks_an_impossible_date() {
        let mut r = rng("jitter-date-impossible");
        let out = jitter_date("2024-13-40", Strength::Default, &mut r);
        assert_ne!(out, "2024-13-40");
        assert_eq!(out.len(), "2024-13-40".len());
    }

    #[test]
    fn jitter_date_redacts_rather_than_leaks_unparseable_free_text() {
        let mut r = rng("jitter-date-unparseable");
        let out = jitter_date("NASCIDO EM 1972", Strength::Default, &mut r);
        assert_ne!(out, "NASCIDO EM 1972");
        assert_eq!(out.len(), "NASCIDO EM 1972".len());
    }

    #[test]
    fn redact_default_preserves_length_and_masks_characters() {
        let out = redact("Ana123", Strength::Default);
        assert_eq!(out.len(), "Ana123".len());
        assert_ne!(out, "Ana123");
    }

    #[test]
    fn redact_maximum_drops_everything() {
        assert_eq!(redact("anything at all", Strength::Maximum), "");
    }

    #[test]
    fn sanitize_strips_mojibake_and_collapses_whitespace() {
        assert_eq!(sanitize("  Sao   Paulo\t"), "Sao Paulo");
        assert_eq!(sanitize("SÃ£o Paulo"), "So Paulo");
    }

    #[test]
    fn mint_key_looks_up_the_ledger_and_returns_the_mapped_value() {
        let class = ClassId(0);
        let mut mapping = BTreeMap::new();
        mapping.insert(KeyValue::Int(1), KeyValue::Int(42));
        let mut mappings = BTreeMap::new();
        mappings.insert(class, mapping);
        let ledger = Ledger { mappings };

        let mut r = rng("mint-key");
        let out = Strategy::MintKey
            .apply(
                "t.id",
                &Value::Int(1),
                Strength::Default,
                &ledger,
                Some(class),
                &mut r,
            )
            .expect("lookup succeeds");
        assert_eq!(out, Value::Int(42));
    }

    #[test]
    fn mint_key_without_a_class_is_an_incompatible_strategy_error() {
        let ledger = Ledger::default();
        let mut r = rng("mint-key-no-class");
        let error = Strategy::MintKey
            .apply(
                "t.notes",
                &Value::Int(1),
                Strength::Default,
                &ledger,
                None,
                &mut r,
            )
            .expect_err("no class means no lookup is possible");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    #[test]
    fn null_values_pass_through_every_strategy_unchanged() {
        let ledger = Ledger::default();
        let mut r = rng("null");
        for strategy in [
            Strategy::Keep,
            Strategy::Redact,
            Strategy::PseudonymizeLocalPart,
            Strategy::PreservePrefix,
            Strategy::RegenerateDocument,
            Strategy::ResampleName,
            Strategy::Jitter,
        ] {
            let out = strategy
                .apply(
                    "t.x",
                    &Value::Null,
                    Strength::Default,
                    &ledger,
                    None,
                    &mut r,
                )
                .expect("null is always representable");
            assert_eq!(out, Value::Null);
        }
    }
}
