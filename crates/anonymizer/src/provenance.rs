//! Per-row provenance: which source rows hold values that read as test,
//! admin or seed fixtures rather than production data.
//!
//! A pure, read-only scan of the source dataset. Its result is counted in
//! the report and the profile and never acted on: nothing here feeds
//! strategy resolution, the ledger, the transform or a compliance check, so
//! a flagged row is anonymized exactly like any other. Passing suspected
//! test rows through needs a reviewed allowlist, because a false positive
//! would release a real person unanonymized.

use std::collections::BTreeMap;

use rayon::prelude::*;
use serde::Serialize;

use crate::classify::{self, SemanticType};
use crate::dataset::{Dataset, Value};

/// Email local parts that name a role, a test or a placeholder rather than a
/// person, compared whole after [`normalised_local_part`].
const PLACEHOLDER_LOCAL_PARTS: &[&str] = &[
    "test",
    "teste",
    "testing",
    "admin",
    "administrator",
    "administrador",
    "root",
    "noreply",
    "no-reply",
    "donotreply",
    "naoresponda",
    "qa",
    "demo",
    "dummy",
    "fake",
    "example",
    "exemplo",
    "sample",
    "fulano",
    "foo",
];

/// Email domains reserved for documentation or run as disposable inboxes,
/// compared whole and case-insensitively.
const PLACEHOLDER_DOMAINS: &[&str] = &[
    "example.com",
    "example.org",
    "example.net",
    "test.com",
    "teste.com",
    "mailinator.com",
    "yopmail.com",
    "guerrillamail.com",
    "sharklasers.com",
    "10minutemail.com",
];

/// Top-level labels reserved by RFC 2606 and RFC 6761: no address under one
/// of them can receive mail.
const RESERVED_TOP_LEVEL_LABELS: &[&str] = &["test", "example", "invalid", "localhost"];

/// Name tokens a person is not called: pt-BR and English stand-ins for "some
/// person", and role words. Compared as whole whitespace-separated tokens.
const PLACEHOLDER_NAME_TOKENS: &[&str] = &[
    "test",
    "teste",
    "testing",
    "fulano",
    "beltrano",
    "ciclano",
    "sicrano",
    "dummy",
    "admin",
    "administrador",
    "usuario",
    "usuário",
];

/// Whole names used as a stand-in for an unknown person.
const PLACEHOLDER_FULL_NAMES: &[&str] = &["john doe", "jane doe"];

/// CPFs printed as examples in widely copied validation tutorials. Any valid
/// CPF could belong to a real person, so this list holds only values whose
/// placeholder use is documented, never a number that merely looks
/// sequential or memorable.
const PUBLISHED_EXAMPLE_CPFS: &[&str] = &["11144477735", "12345678909"];

/// CNPJs printed as examples in widely copied validation tutorials, on the
/// same terms as [`PUBLISHED_EXAMPLE_CPFS`].
const PUBLISHED_EXAMPLE_CNPJS: &[&str] = &["11222333000181", "12345678000195"];

/// Digit counts of a CPF and a CNPJ.
const CPF_DIGITS: usize = 11;
const CNPJ_DIGITS: usize = 14;

/// The characters a text document may hold besides digits for it to be read
/// as a document at all.
const DOCUMENT_PUNCTUATION: &[char] = &['.', '-', '/', ' '];

/// Brazil's country calling code, dropped from a phone number of 12 or 13
/// digits before its area code is read.
const BRAZIL_CALLING_CODE: &str = "55";

/// Why a field reads as a placeholder rather than production data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum NonProductionSignal {
    /// An email whose local part is a role or test word (`admin@`, `test+1@`).
    PlaceholderLocalPart,
    /// An email at a documentation, disposable-inbox or reserved domain.
    PlaceholderDomain,
    /// A full name holding a stand-in token (`Fulano`, `Test`) or a stand-in
    /// whole name.
    PlaceholderName,
    /// A phone number with area code `00`, or whose subscriber digits are
    /// all the same.
    PlaceholderPhone,
    /// A CPF or CNPJ that is one digit repeated, or a published example.
    PlaceholderDocument,
}

/// The non-production rows of one table. Counts only: never a value, never a
/// row position, and never acted on.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct NonProductionRows {
    /// Rows holding at least one field that reads as a placeholder.
    pub flagged: usize,
    /// Of `flagged`, rows that also hold a well-formed direct identifier on
    /// which no signal fired: most likely a real person with a placeholder
    /// typed into one field, and the rows worth reviewing first.
    pub mixed: usize,
    /// `table.column` -> signal -> number of cells.
    pub fields: BTreeMap<String, BTreeMap<NonProductionSignal, usize>>,
}

/// The signals one cell carries, at most two (an email can have both a
/// placeholder local part and a placeholder domain).
type CellSignals = [Option<NonProductionSignal>; 2];

/// How one scanned cell bears on its row's provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellReading {
    Placeholder(CellSignals),
    /// Non-null, no signal, and well-formed for its column's type.
    RealLooking,
    /// NULL, malformed, or not a shape the scan reads.
    Silent,
}

/// Scans every column classified [`SemanticType::Email`],
/// [`SemanticType::FullName`], [`SemanticType::Phone`],
/// [`SemanticType::Cpf`] or [`SemanticType::Cnpj`], keyed by `table.column`
/// in `semantic_types`, and counts each table's non-production rows.
///
/// A table appears only when at least one of its rows is flagged, so a clean
/// dataset yields an empty map. A row is judged within its own table: a
/// placeholder in a parent row never makes a child row flagged or mixed.
pub fn scan(
    dataset: &Dataset,
    semantic_types: &BTreeMap<String, SemanticType>,
) -> BTreeMap<String, NonProductionRows> {
    let readings: BTreeMap<String, Vec<CellReading>> = dataset
        .qualified_columns()
        .par_iter()
        .filter_map(|(name, column)| {
            let semantic_type = *semantic_types.get(name)?;
            is_scanned(semantic_type).then(|| {
                let cells = column
                    .values
                    .iter()
                    .map(|value| read_cell(semantic_type, value))
                    .collect();
                (name.clone(), cells)
            })
        })
        .collect();

    let mut tables = BTreeMap::new();
    for table in &dataset.tables {
        let columns: Vec<(String, &[CellReading])> = table
            .columns
            .iter()
            .filter_map(|column| {
                let name = column.qualified_name(&table.name);
                let cells = readings.get(&name)?;
                Some((name, cells.as_slice()))
            })
            .collect();
        let rows = fold_rows(&columns, table.row_count());
        if rows.flagged > 0 {
            tables.insert(table.name.clone(), rows);
        }
    }
    tables
}

fn is_scanned(semantic_type: SemanticType) -> bool {
    matches!(
        semantic_type,
        SemanticType::Email
            | SemanticType::FullName
            | SemanticType::Phone
            | SemanticType::Cpf
            | SemanticType::Cnpj
    )
}

fn fold_rows(columns: &[(String, &[CellReading])], row_count: usize) -> NonProductionRows {
    let mut rows = NonProductionRows::default();
    for row in 0..row_count {
        let mut flagged = false;
        let mut real_looking = false;
        for (name, cells) in columns {
            match cells.get(row).copied().unwrap_or(CellReading::Silent) {
                CellReading::Placeholder(signals) => {
                    flagged = true;
                    let field = rows.fields.entry(name.clone()).or_default();
                    for signal in signals.into_iter().flatten() {
                        *field.entry(signal).or_default() += 1;
                    }
                }
                CellReading::RealLooking => real_looking = true,
                CellReading::Silent => {}
            }
        }
        if flagged {
            rows.flagged += 1;
            if real_looking {
                rows.mixed += 1;
            }
        }
    }
    rows
}

fn read_cell(semantic_type: SemanticType, value: &Value) -> CellReading {
    let signals = cell_signals(semantic_type, value);
    if signals.iter().any(Option::is_some) {
        return CellReading::Placeholder(signals);
    }
    let well_formed = match value {
        Value::Text(text) => semantic_type.matches_text(text.trim()),
        Value::Int(_) => classify::is_valid_document(semantic_type, value),
        Value::Null | Value::Bool(_) | Value::Float(_) => false,
    };
    if well_formed {
        CellReading::RealLooking
    } else {
        CellReading::Silent
    }
}

fn cell_signals(semantic_type: SemanticType, value: &Value) -> CellSignals {
    match (semantic_type, value) {
        (SemanticType::Email, Value::Text(text)) => email_signals(text),
        (SemanticType::FullName, Value::Text(text)) => [
            is_placeholder_name(text).then_some(NonProductionSignal::PlaceholderName),
            None,
        ],
        (SemanticType::Phone, Value::Text(text)) => [
            is_placeholder_phone(text).then_some(NonProductionSignal::PlaceholderPhone),
            None,
        ],
        (SemanticType::Cpf | SemanticType::Cnpj, _) => [
            is_placeholder_document(semantic_type, value)
                .then_some(NonProductionSignal::PlaceholderDocument),
            None,
        ],
        _ => [None, None],
    }
}

fn email_signals(text: &str) -> CellSignals {
    let Some((local, domain)) = text.trim().rsplit_once('@') else {
        return [None, None];
    };
    let local_part = PLACEHOLDER_LOCAL_PARTS
        .contains(&normalised_local_part(local).as_str())
        .then_some(NonProductionSignal::PlaceholderLocalPart);
    let domain = is_placeholder_domain(domain).then_some(NonProductionSignal::PlaceholderDomain);
    [local_part, domain]
}

/// Lowercased, cut at the first `+` (a sub-address), and stripped of a
/// trailing run of ASCII digits and `.`, `_` or `-`, so `Test+qa@`,
/// `admin2@` and `teste_01@` all read as their bare word.
fn normalised_local_part(local: &str) -> String {
    let lower = local.to_lowercase();
    let before_tag = lower.split('+').next().unwrap_or_default();
    before_tag
        .trim_end_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        .to_owned()
}

fn is_placeholder_domain(domain: &str) -> bool {
    let domain = domain.to_lowercase();
    PLACEHOLDER_DOMAINS.contains(&domain.as_str())
        || RESERVED_TOP_LEVEL_LABELS.iter().any(|label| {
            domain == *label
                || domain
                    .strip_suffix(label)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

fn is_placeholder_name(text: &str) -> bool {
    let lower = text.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    tokens
        .iter()
        .any(|token| PLACEHOLDER_NAME_TOKENS.contains(token))
        || PLACEHOLDER_FULL_NAMES.contains(&tokens.join(" ").as_str())
}

/// The subscriber digits of a phone number, after dropping a leading
/// calling code from a 12- or 13-digit number, together with its area code
/// when it has one. `None` for any text holding a letter or of a length no
/// Brazilian number has.
fn phone_parts(text: &str) -> Option<(Option<String>, String)> {
    if text.chars().any(char::is_alphabetic) {
        return None;
    }
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let national = match digits.len() {
        12 | 13 => digits.strip_prefix(BRAZIL_CALLING_CODE)?.to_owned(),
        _ => digits,
    };
    match national.len() {
        10 | 11 => {
            let (area, subscriber) = national.split_at(2);
            Some((Some(area.to_owned()), subscriber.to_owned()))
        }
        8 | 9 => Some((None, national)),
        _ => None,
    }
}

fn is_placeholder_phone(text: &str) -> bool {
    phone_parts(text).is_some_and(|(area, subscriber)| {
        area.as_deref() == Some("00") || is_one_digit_repeated(&subscriber)
    })
}

fn is_one_digit_repeated(digits: &str) -> bool {
    let mut chars = digits.chars();
    chars
        .next()
        .is_some_and(|first| chars.all(|digit| digit == first))
}

/// The digits of a document cell at its type's width: a text cell holding
/// only digits and [`DOCUMENT_PUNCTUATION`], or a non-negative integer
/// zero-padded to restore the leading zeros integer storage drops.
fn document_digits(value: &Value, width: usize) -> Option<String> {
    let digits = match value {
        Value::Text(text) => {
            let text = text.trim();
            if !text
                .chars()
                .all(|c| c.is_ascii_digit() || DOCUMENT_PUNCTUATION.contains(&c))
            {
                return None;
            }
            text.chars().filter(char::is_ascii_digit).collect()
        }
        Value::Int(n) if *n >= 0 => format!("{n:0width$}"),
        _ => return None,
    };
    (digits.len() == width).then_some(digits)
}

fn is_placeholder_document(semantic_type: SemanticType, value: &Value) -> bool {
    let (width, published) = match semantic_type {
        SemanticType::Cpf => (CPF_DIGITS, PUBLISHED_EXAMPLE_CPFS),
        SemanticType::Cnpj => (CNPJ_DIGITS, PUBLISHED_EXAMPLE_CNPJS),
        _ => return false,
    };
    document_digits(value, width).is_some_and(|digits| {
        is_one_digit_repeated(&digits) || published.contains(&digits.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_example_cpf_is_check_digit_valid() {
        for cpf in PUBLISHED_EXAMPLE_CPFS {
            assert!(classify::is_valid_cpf(cpf), "{cpf}");
        }
    }

    #[test]
    fn every_published_example_cnpj_is_check_digit_valid() {
        for cnpj in PUBLISHED_EXAMPLE_CNPJS {
            assert!(classify::is_valid_cnpj(cnpj), "{cnpj}");
        }
    }

    #[test]
    fn a_local_part_is_read_without_its_tag_and_trailing_counter() {
        assert_eq!(normalised_local_part("Test+signup"), "test");
        assert_eq!(normalised_local_part("admin2"), "admin");
        assert_eq!(normalised_local_part("teste_01"), "teste");
        assert_eq!(normalised_local_part("qa.-"), "qa");
        assert_eq!(normalised_local_part("no-reply"), "no-reply");
        assert_eq!(normalised_local_part("2test"), "2test");
        assert_eq!(normalised_local_part("badminton"), "badminton");
    }

    #[test]
    fn a_phone_drops_its_calling_code_before_its_area_code_is_read() {
        assert_eq!(
            phone_parts("+55 (11) 98765-4321"),
            Some((Some("11".to_owned()), "987654321".to_owned()))
        );
        assert_eq!(
            phone_parts("(55) 3333-4444"),
            Some((Some("55".to_owned()), "33334444".to_owned()))
        );
        assert_eq!(
            phone_parts("99999-9999"),
            Some((None, "999999999".to_owned()))
        );
        assert_eq!(phone_parts("ramal 22"), None);
        assert_eq!(phone_parts("123"), None);
    }

    #[test]
    fn a_phone_with_area_code_zero_or_one_repeated_subscriber_digit_is_a_placeholder() {
        assert!(is_placeholder_phone("(00) 91234-5678"));
        assert!(is_placeholder_phone("+55 11 99999-9999"));
        assert!(is_placeholder_phone("0000-0000"));
        assert!(!is_placeholder_phone("(11) 99999-9998"));
        assert!(!is_placeholder_phone("(11) 3333-4444"));
    }
}
