pub mod ledger;

use std::collections::BTreeMap;
use std::ops::Range;

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::classify::SemanticType;
use crate::dataset::{ColumnSpec, ColumnType, KeyValue, Value};
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
/// domain of its own to keep (an address with no `@`, or with text after it
/// that is not a domain).
const FALLBACK_EMAIL_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];

/// The characters [`random_token`] draws a generated local part from.
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// What the pipeline knows about a column beyond its declared type: how it was
/// classified, and the equivalence class its keys mint in, if it is a key.
#[derive(Debug, Clone, Copy)]
pub struct ColumnRole {
    pub semantic_type: SemanticType,
    pub class: Option<ClassId>,
}

/// Every transform but [`Strategy::Shuffle`] is local to the value and
/// preserves the part that carries the distribution; `Shuffle` moves whole
/// values between rows and keeps the column's multiset. Either way fidelity
/// holds by construction rather than by resampling. Enum dispatch: no
/// `dyn Strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
    /// Minted through the ledger's per-class domain.
    MintKey,
    /// Minted through the ledger's dataset-wide UUID domain, independent of
    /// any equivalence class.
    MintUuid,
    /// Minted through the ledger's dataset-wide handle domain: a fresh
    /// lowercase handle of 8 to 32 characters, the same for a source value
    /// wherever it occurs, never equal to any source value.
    MintHandle,
    /// Permuted across the column's non-null rows by a stream seeded on the
    /// run and the column: every value is kept whole, nulls stay in place, and
    /// the column's multiset of values is exactly the source's. The one
    /// strategy that is not value-local; see [`shuffle_values`].
    Shuffle,
}

impl Strategy {
    /// The strategy a column falls back to when [`resolve`] finds no
    /// override: a function of the column's semantic type alone. A key
    /// column whose semantic type is not [`SemanticType::Uuid`] never
    /// reaches this — [`resolve`] dispatches it to [`Strategy::MintKey`] via
    /// [`crate::dataset::ColumnSpec::is_key`] before semantic type is
    /// otherwise consulted, since a numeric column that happens to be a
    /// primary or foreign key must mint through the ledger, not jitter like
    /// an ordinary measurement. [`SemanticType::Uuid`] always resolves to
    /// [`Strategy::MintUuid`] here regardless of key status, which is what
    /// lets [`resolve`] route a declared key the same way it routes an
    /// undeclared one.
    pub fn for_semantic_type(semantic_type: SemanticType) -> Strategy {
        match semantic_type {
            SemanticType::Email => Strategy::PseudonymizeLocalPart,
            SemanticType::Uuid => Strategy::MintUuid,
            SemanticType::Phone | SemanticType::Cep => Strategy::PreservePrefix,
            SemanticType::Cpf | SemanticType::Cnpj => Strategy::RegenerateDocument,
            SemanticType::FullName => Strategy::ResampleName,
            SemanticType::Username => Strategy::MintHandle,
            SemanticType::Date | SemanticType::NumericId => Strategy::Jitter,
            SemanticType::FreeText | SemanticType::Unknown => Strategy::Redact,
        }
    }

    /// The part of `output` this strategy generated, as opposed to kept from
    /// the source: the leakage check looks for source words only there. See
    /// [`Strategy::generated_range`].
    pub fn generated_part(self, output: &str) -> &str {
        &output[self.generated_range(output)]
    }

    /// The byte range of `output` this strategy generated, as opposed to
    /// kept from the source. [`Strategy::PseudonymizeLocalPart`] keeps the
    /// domain after the last `@`, so it generated what precedes it.
    /// [`Strategy::PreservePrefix`] keeps the leading digits of a value
    /// holding no letter, so it generated what follows the most digits it
    /// keeps at any strength (the [`Strength::Default`] prefix); a value
    /// holding a letter it redacts whole. Every other strategy generates the
    /// whole value.
    pub fn generated_range(self, output: &str) -> Range<usize> {
        match self {
            Strategy::PseudonymizeLocalPart => 0..output.rfind('@').unwrap_or(output.len()),
            Strategy::PreservePrefix if !output.chars().any(char::is_alphabetic) => {
                let digits = output.chars().filter(char::is_ascii_digit).count();
                let kept = prefix_length(digits, Strength::Default);
                let start = output
                    .char_indices()
                    .filter(|(_, c)| c.is_ascii_digit())
                    .take(kept)
                    .last()
                    .map_or(0, |(index, c)| index + c.len_utf8());
                start..output.len()
            }
            Strategy::Keep
            | Strategy::Redact
            | Strategy::PreservePrefix
            | Strategy::RegenerateDocument
            | Strategy::ResampleName
            | Strategy::Jitter
            | Strategy::MintKey
            | Strategy::MintUuid
            | Strategy::MintHandle
            | Strategy::Shuffle => 0..output.len(),
        }
    }

    /// Whether a word of the source surviving into the output is evidence of
    /// a leak. False where the output is drawn without regard to the source's
    /// words ([`Strategy::ResampleName`], [`Strategy::RegenerateDocument`],
    /// [`Strategy::MintKey`], [`Strategy::MintUuid`], [`Strategy::MintHandle`]),
    /// where every letter is
    /// replaced ([`Strategy::Redact`]), and where the value is returned whole
    /// and the whole-value comparison already decides ([`Strategy::Keep`],
    /// [`Strategy::Shuffle`]).
    pub fn source_words_surviving_is_a_leak(self) -> bool {
        match self {
            Strategy::PseudonymizeLocalPart | Strategy::PreservePrefix | Strategy::Jitter => true,
            Strategy::Keep
            | Strategy::Redact
            | Strategy::RegenerateDocument
            | Strategy::ResampleName
            | Strategy::MintKey
            | Strategy::MintUuid
            | Strategy::MintHandle
            | Strategy::Shuffle => false,
        }
    }

    /// Applies this strategy to one value of `column`, used only to name
    /// `column` in [`Error::IncompatibleStrategy`] should this strategy and
    /// `value`'s shape turn out incompatible (a `MintKey` override on a
    /// non-key column, say). `rng` must be derived per value, keyed on
    /// whatever identifies the value for this strategy's consistency
    /// requirements — [`crate::context::RunContext::rng_for_row_position`]
    /// for every strategy here except [`Strategy::MintKey`], which instead
    /// looks up an already-minted value and draws no randomness of its own. `role` carries the
    /// column's classification, which [`Strategy::RegenerateDocument`] on an integer
    /// reads to choose between a CPF and a CNPJ, and the ledger class
    /// [`Strategy::MintKey`] looks up. [`Strategy::Shuffle`] moves values
    /// between rows and never changes one, so per value it returns `value`
    /// unchanged; the permutation itself is [`shuffle_values`].
    pub fn apply(
        self,
        column: &str,
        value: &Value,
        strength: Strength,
        role: ColumnRole,
        ledger: &Ledger,
        rng: &mut ChaCha8Rng,
    ) -> Result<Value, Error> {
        if value.is_null() {
            return Ok(Value::Null);
        }

        if self == Strategy::MintKey {
            return mint_key(column, value, ledger, role.class);
        }
        if self == Strategy::MintUuid {
            return mint_uuid(column, value, ledger, strength);
        }
        if self == Strategy::MintHandle {
            return mint_handle(column, value, ledger);
        }

        match (self, value) {
            (Strategy::Keep | Strategy::Shuffle, _) => Ok(value.clone()),
            (Strategy::Redact, Value::Text(text)) => Ok(Value::Text(redact(text, strength))),
            (Strategy::Redact, other) => Ok(other.clone()),
            (Strategy::PseudonymizeLocalPart, Value::Text(text)) => Ok(Value::Text(
                pseudonymize_email(text, &sanitize(text), strength, rng),
            )),
            (Strategy::PreservePrefix, Value::Text(text)) => Ok(Value::Text(preserve_prefix(
                text,
                &sanitize(text),
                strength,
                rng,
            ))),
            (Strategy::RegenerateDocument, Value::Text(text)) => Ok(Value::Text(
                regenerate_document(&sanitize(text), strength, rng),
            )),
            (Strategy::RegenerateDocument, Value::Int(_)) => Ok(Value::Int(
                regenerate_integer_document(role.semantic_type, rng),
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
/// `overrides` names one, otherwise [`Strategy::MintUuid`] for a column
/// classified [`SemanticType::Uuid`], [`Strategy::MintKey`] for any other
/// key column, or [`Strategy::for_semantic_type`] for everything else.
///
/// Rejects a combination that cannot work: `MintKey` requires a ledger class
/// and therefore a key column, so it is refused on a non-key one.
/// Conversely a key column requires a strategy that actually mints it —
/// `MintKey` or `MintUuid` — since anything else would quietly break
/// referential integrity by leaving a key unminted. A strategy that
/// [`Strategy::apply`] has no arm for against `spec.ty` is rejected here
/// too, so an incompatible override surfaces as a validation error before
/// any row runs, never as a panic-shaped failure mid-transform.
pub(crate) fn resolve(
    overrides: &BTreeMap<String, Strategy>,
    qualified: &str,
    spec: &ColumnSpec,
    semantic_type: SemanticType,
) -> Result<Strategy, Error> {
    let is_key = spec.is_key();
    let strategy = overrides.get(qualified).copied().unwrap_or_else(|| {
        if semantic_type == SemanticType::Uuid {
            Strategy::MintUuid
        } else if is_key {
            Strategy::MintKey
        } else {
            Strategy::for_semantic_type(semantic_type)
        }
    });

    let incompatible = || Error::IncompatibleStrategy {
        column: qualified.to_owned(),
        strategy,
    };

    if strategy == Strategy::MintKey && !is_key {
        return Err(incompatible());
    }
    if is_key && !matches!(strategy, Strategy::MintKey | Strategy::MintUuid) {
        return Err(incompatible());
    }
    if !type_compatible(strategy, spec.ty) {
        return Err(incompatible());
    }
    Ok(strategy)
}

/// Whether [`Strategy::apply`] has a match arm that can run `strategy`
/// against a column declared `ty`, independent of any particular value.
/// Mirrors `apply`'s arms exactly: a strategy and type this says are
/// compatible never falls through to `apply`'s catch-all error, and one
/// this says are not never reaches `apply` at all, since [`resolve`] checks
/// every column before the transform stage runs any of them.
pub(crate) fn type_compatible(strategy: Strategy, ty: ColumnType) -> bool {
    match strategy {
        Strategy::Keep | Strategy::Redact | Strategy::Shuffle => true,
        Strategy::MintKey => matches!(ty, ColumnType::Integer | ColumnType::Text),
        Strategy::MintUuid | Strategy::MintHandle => ty == ColumnType::Text,
        Strategy::RegenerateDocument => matches!(ty, ColumnType::Integer | ColumnType::Text),
        Strategy::PseudonymizeLocalPart | Strategy::PreservePrefix | Strategy::ResampleName => {
            ty == ColumnType::Text
        }
        Strategy::Jitter => matches!(
            ty,
            ColumnType::Integer | ColumnType::Float | ColumnType::Text
        ),
    }
}

/// The most distinct non-null values a column may hold and still be
/// shuffled. A column this small is a vocabulary (a status, a state, a
/// rating), whose values carry the distribution and identify nobody on
/// their own; above it, values start to behave like identifiers, which a
/// permutation would hand to another row intact.
pub const SHUFFLE_MAX_DISTINCT: usize = 20;

/// The fewest rows every distinct value of a shuffled column must occur in.
/// A value seen once or twice is rare enough to point at the row it came
/// from, and a permutation only moves it to another row rather than hiding
/// it, so a column holding such a value keeps its default strategy.
pub const SHUFFLE_MIN_OCCURRENCES: usize = 3;

/// The fewest distinct non-null values a shuffled column must hold. A
/// single-valued column comes out of a permutation unchanged, so shuffling
/// it would publish its one value as it stands.
pub const SHUFFLE_MIN_DISTINCT: usize = 2;

/// Column-name words, and word sequences, that suggest the column holds a
/// person or a member of staff. A column whose name contains one as whole
/// tokens is never shuffled: a few-valued column of staff logins, sellers,
/// doctors or authors the classifier does not recognise as a direct
/// identifier would otherwise pass through moved between rows rather than
/// masked, and each value still names someone. Matching whole tokens of the
/// name split on `_`, `-`, spaces and camelCase keeps `username`,
/// `created_by` and `vendedorId` out while `status` stays shuffleable.
pub const PERSON_NAME_TOKENS: &[&str] = &[
    "user",
    "username",
    "usuario",
    "login",
    "created_by",
    "updated_by",
    "modified_by",
    "author",
    "autor",
    "owner",
    "dono",
    "vendedor",
    "atendente",
    "agente",
    "agent",
    "operador",
    "operator",
    "medico",
    "doctor",
    "responsavel",
    "funcionario",
    "employee",
    "cliente",
    "customer",
    "paciente",
    "patient",
    "nome",
    "name",
    "member",
    "membro",
    "alias",
    "apelido",
    "nick",
    "nickname",
];

/// The lowercase tokens of a column name, split on every non-alphanumeric
/// character and at each lowercase-or-digit to uppercase boundary.
fn name_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Whether `name` holds an entry of [`PERSON_NAME_TOKENS`] as a run of
/// whole tokens (`created_by` as `created`, `by`).
fn name_suggests_a_person(name: &str) -> bool {
    let tokens = name_tokens(name);
    PERSON_NAME_TOKENS.iter().any(|entry| {
        let wanted: Vec<&str> = entry.split('_').collect();
        tokens.windows(wanted.len()).any(|window| {
            window
                .iter()
                .zip(&wanted)
                .all(|(token, wanted)| token == wanted)
        })
    })
}

/// Whether a column whose strategy would otherwise be `default` runs under
/// [`Strategy::Shuffle`] instead: `default` is [`Strategy::Redact`] on a
/// `Text` column (on any other type `Redact` returns the value unchanged,
/// so a Boolean flag keeps its row), or [`Strategy::Jitter`] on an
/// `Integer` column; the column is no key and
/// in no equivalence class (`in_class`); it is no direct identifier; its
/// name suggests no person ([`PERSON_NAME_TOKENS`]); and its non-null
/// values are low-cardinality by [`SHUFFLE_MIN_DISTINCT`],
/// [`SHUFFLE_MAX_DISTINCT`] and [`SHUFFLE_MIN_OCCURRENCES`]. The caller decides overrides and
/// preservation first, since either names the strategy outright.
pub(crate) fn qualifies_for_shuffle(
    default: Strategy,
    spec: &ColumnSpec,
    in_class: bool,
    semantic_type: SemanticType,
    values: &[Value],
) -> bool {
    let replaceable = match default {
        Strategy::Redact => spec.ty == ColumnType::Text,
        Strategy::Jitter => spec.ty == ColumnType::Integer,
        _ => false,
    };
    replaceable
        && !spec.is_key()
        && !in_class
        && !semantic_type.is_direct_identifier()
        && !name_suggests_a_person(&spec.name)
        && is_low_cardinality(values)
}

/// Whether `values` hold at least [`SHUFFLE_MIN_DISTINCT`] and at most
/// [`SHUFFLE_MAX_DISTINCT`] distinct non-null values, each of them in at
/// least [`SHUFFLE_MIN_OCCURRENCES`] rows.
fn is_low_cardinality(values: &[Value]) -> bool {
    let mut counts: BTreeMap<ShuffleKey<'_>, usize> = BTreeMap::new();
    for value in values {
        let Some(key) = ShuffleKey::of(value) else {
            continue;
        };
        *counts.entry(key).or_insert(0) += 1;
        if counts.len() > SHUFFLE_MAX_DISTINCT {
            return false;
        }
    }
    counts.len() >= SHUFFLE_MIN_DISTINCT
        && counts
            .values()
            .all(|&count| count >= SHUFFLE_MIN_OCCURRENCES)
}

/// A non-null cell's identity for counting distinct values: floats by their
/// bit pattern, so the key is totally ordered.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ShuffleKey<'a> {
    Bool(bool),
    Int(i64),
    Float(u64),
    Text(&'a str),
}

impl<'a> ShuffleKey<'a> {
    fn of(value: &'a Value) -> Option<Self> {
        match value {
            Value::Null => None,
            Value::Bool(b) => Some(ShuffleKey::Bool(*b)),
            Value::Int(n) => Some(ShuffleKey::Int(*n)),
            Value::Float(f) => Some(ShuffleKey::Float(f.to_bits())),
            Value::Text(text) => Some(ShuffleKey::Text(text)),
        }
    }
}

/// [`Strategy::Shuffle`]'s permutation: the non-null values of `values`,
/// reordered by a Fisher-Yates pass over `rng`, written back into the
/// non-null rows in row order. Nulls keep their rows and the multiset of
/// values is unchanged. `rng` must be keyed on the run seed and the column
/// alone, so the permutation never depends on how work was scheduled.
pub(crate) fn shuffle_values(values: &[Value], rng: &mut ChaCha8Rng) -> Vec<Value> {
    let mut pool: Vec<Value> = values.iter().filter(|v| !v.is_null()).cloned().collect();
    for upper in (1..pool.len()).rev() {
        let pick = rng.random_range(0..=upper);
        pool.swap(upper, pick);
    }
    let mut drawn = pool.into_iter();
    values
        .iter()
        .map(|value| {
            if value.is_null() {
                Value::Null
            } else {
                drawn.next().unwrap_or(Value::Null)
            }
        })
        .collect()
}

/// The closed range of a column's numeric source values, by variant. Every
/// `Int` and every `Float` cell a [`Strategy::Jitter`] column's transform
/// and re-injection write is held inside the range of its own variant, so a
/// jittered or re-corrupted number never leaves the magnitudes the source
/// column held, and an integer stays an integer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct SourceRange {
    int: Option<(i64, i64)>,
    float: Option<(f64, f64)>,
}

impl SourceRange {
    /// The range of the non-null numeric cells of `source`.
    pub(crate) fn of(source: &[Value]) -> Self {
        let mut range = SourceRange::default();
        for value in source {
            match value {
                Value::Int(n) => {
                    let (low, high) = range.int.unwrap_or((*n, *n));
                    range.int = Some((low.min(*n), high.max(*n)));
                }
                Value::Float(f) if f.is_finite() => {
                    let (low, high) = range.float.unwrap_or((*f, *f));
                    range.float = Some((low.min(*f), high.max(*f)));
                }
                _ => {}
            }
        }
        range
    }

    /// Brings every numeric cell of `values` inside the source range of its
    /// variant by folding it back across the bound it crossed: `v > max`
    /// becomes `max - (v - max)` and `v < min` becomes `min + (min - v)`,
    /// then a fold that overshoots the opposite bound is clamped to it.
    /// Folding rather than pinning keeps cells that jittered past a bound
    /// spread out near it, so they neither pile up on the extreme value nor
    /// hand the row holding it its own source back. An integer whose fold
    /// lands exactly on its own row's value in `source` moves one step
    /// further inside the range, since a jittered value folding back onto
    /// its source would return it unchanged. A cell of a variant the source
    /// never held, and every non-numeric cell, is left as it is.
    pub(crate) fn fold_into(&self, values: &mut [Value], source: &[Value]) {
        for (row, value) in values.iter_mut().enumerate() {
            match value {
                Value::Int(n) => {
                    if let Some((low, high)) = self.int {
                        let folded = fold_int(*n, low, high);
                        let crossed = *n != folded;
                        *n = if crossed && source.get(row) == Some(&Value::Int(folded)) {
                            step_inside(folded, low, high)
                        } else {
                            folded
                        };
                    }
                }
                Value::Float(f) => {
                    if let Some((low, high)) = self.float {
                        *f = fold_float(*f, low, high);
                    }
                }
                _ => {}
            }
        }
    }
}

/// `n` folded into `[low, high]`; see [`SourceRange::fold_into`]. Computed
/// in `i128`, so no fold overflows.
fn fold_int(n: i64, low: i64, high: i64) -> i64 {
    let (n, low_wide, high_wide) = (i128::from(n), i128::from(low), i128::from(high));
    let folded = if n > high_wide {
        high_wide - (n - high_wide)
    } else if n < low_wide {
        low_wide + (low_wide - n)
    } else {
        n
    };
    i64::try_from(folded.clamp(low_wide, high_wide)).unwrap_or(low)
}

/// `n` moved one step toward the middle of `[low, high]`, or `n` itself
/// when the range holds a single value.
fn step_inside(n: i64, low: i64, high: i64) -> i64 {
    let above_low = i128::from(n) - i128::from(low);
    let below_high = i128::from(high) - i128::from(n);
    if above_low >= below_high && n > low {
        n - 1
    } else if n < high {
        n + 1
    } else {
        n
    }
}

/// `f` folded into `[low, high]`; see [`SourceRange::fold_into`]. A NaN
/// becomes `low`.
fn fold_float(f: f64, low: f64, high: f64) -> f64 {
    if f.is_nan() {
        return low;
    }
    let folded = if f > high {
        high - (f - high)
    } else if f < low {
        low + (low - f)
    } else {
        f
    };
    if folded.is_nan() || folded < low {
        low
    } else if folded > high {
        high
    } else {
        folded
    }
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

/// Looks up `value`'s minted counterpart in the dataset-wide UUID space. A
/// value absent from the ledger is one [`crate::transform::ledger::LedgerBuilder::observe_dataset`]
/// never observed because it was not UUID-shaped to begin with — a stray
/// malformed entry in an otherwise `MintUuid`-routed column — so it falls
/// back to [`redact`] rather than erroring: it has no identity in this
/// space to preserve, but it must not survive into the output verbatim
/// either.
fn mint_uuid(
    column: &str,
    value: &Value,
    ledger: &Ledger,
    strength: Strength,
) -> Result<Value, Error> {
    let Value::Text(text) = value else {
        return Err(Error::IncompatibleStrategy {
            column: column.to_owned(),
            strategy: Strategy::MintUuid,
        });
    };
    match ledger.lookup_uuid(text) {
        Some(minted) => Ok(Value::Text(minted.to_owned())),
        None => Ok(Value::Text(redact(text, strength))),
    }
}

/// Looks up `value`'s minted counterpart in the dataset-wide handle space.
/// [`crate::transform::ledger::LedgerBuilder::observe_handle_columns`] observes
/// every text value of a column routed here, so a miss means the column was
/// never observed, which is an error rather than a reason to emit anything.
fn mint_handle(column: &str, value: &Value, ledger: &Ledger) -> Result<Value, Error> {
    let incompatible = || Error::IncompatibleStrategy {
        column: column.to_owned(),
        strategy: Strategy::MintHandle,
    };
    let Value::Text(text) = value else {
        return Err(incompatible());
    };
    ledger
        .lookup_handle(text)
        .map(|minted| Value::Text(minted.to_owned()))
        .ok_or_else(incompatible)
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
        Strength::Default => {
            let masked: String = sanitized.chars().map(mask_char).collect();
            ensure_changed(&sanitized, masked)
        }
        Strength::Aggressive => "REDACTED".to_owned(),
        Strength::Maximum => String::new(),
    }
}

/// Redraws any digit — ASCII or otherwise — to `'0'`, and any letter to a
/// fixed ASCII stand-in: `'X'` for an uppercase or caseless letter (CJK and
/// similar scripts carry no case of their own), `'x'` for a lowercase one.
/// Every other character passes through unchanged.
fn mask_char(c: char) -> char {
    if c.is_numeric() {
        '0'
    } else if c.is_alphabetic() {
        if c.is_lowercase() { 'x' } else { 'X' }
    } else {
        c
    }
}

/// Guarantees redaction actually changes a non-empty value, which
/// character-class masking alone does not: a document made entirely of
/// zeros, a run of the same letter, or punctuation alone each mask to
/// themselves, since [`mask_char`] maps every character of its own class to
/// one fixed representative. Flipping every character of such a result,
/// rather than only one, is what keeps a masked document from passing its
/// own check digits by coincidence.
fn ensure_changed(original: &str, masked: String) -> String {
    if original.is_empty() || masked != original {
        return masked;
    }
    masked.chars().map(flip).collect()
}

/// A character guaranteed to differ from `c`, for every possible `c`.
fn flip(c: char) -> char {
    match c {
        '0' => '1',
        c if c.is_ascii_digit() => '0',
        'X' => 'Y',
        c if c.is_uppercase() => 'X',
        'x' => 'y',
        c if c.is_lowercase() => 'x',
        '*' => '#',
        _ => '*',
    }
}

/// The domain of an address: what follows its last `@`, when that is not
/// empty and consists only of alphanumeric characters, `.` and `-`. Anything
/// else after the `@` is text, not a domain, and is not kept.
fn email_domain(text: &str) -> Option<&str> {
    text.rsplit_once('@')
        .map(|(_, domain)| domain)
        .filter(|domain| !domain.is_empty())
        .filter(|domain| {
            domain
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        })
}

/// A fresh local part at `@` and the domain of `text`, the sanitized form of
/// the cell `source`. A drawn local part that keeps a run of `source` in its
/// source position is moved off it (see [`steer_local_part_off_source`]).
fn pseudonymize_email(
    source: &str,
    text: &str,
    strength: Strength,
    rng: &mut ChaCha8Rng,
) -> String {
    let domain = email_domain(text).map(str::to_owned).unwrap_or_else(|| {
        FALLBACK_EMAIL_DOMAINS[rng.random_range(0..FALLBACK_EMAIL_DOMAINS.len())].to_owned()
    });
    let local = random_token(local_part_len(strength), rng);
    steer_local_part_off_source(source, &local, &domain)
}

/// The address `{local}@{domain}`, with each character of `local` at which
/// [`crate::compliance::survival_in_place`] finds `source` surviving moved to
/// the next character of [`ALPHABET`], until nothing survives or every
/// character has had a turn. Draws no randomness, so an address that keeps
/// nothing of its source is exactly the draw, at every strength.
fn steer_local_part_off_source(source: &str, local: &str, domain: &str) -> String {
    let mut local: Vec<char> = local.chars().collect();
    for _ in 0..local.len() {
        let address = format!("{}@{domain}", local.iter().collect::<String>());
        let Some(offset) = crate::compliance::survival_in_place(
            source,
            &address,
            Some(Strategy::PseudonymizeLocalPart),
        ) else {
            return address;
        };
        let Some(c) = local.get_mut(offset) else {
            return address;
        };
        *c = next_in_alphabet(*c);
    }
    format!("{}@{domain}", local.iter().collect::<String>())
}

fn next_in_alphabet(c: char) -> char {
    let next = ALPHABET
        .iter()
        .position(|&letter| char::from(letter) == c)
        .map_or(0, |index| (index + 1) % ALPHABET.len());
    char::from(ALPHABET[next])
}

fn local_part_len(strength: Strength) -> usize {
    match strength {
        Strength::Default => 10,
        Strength::Aggressive => 6,
        Strength::Maximum => 4,
    }
}

fn random_token(len: usize, rng: &mut ChaCha8Rng) -> String {
    (0..len)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Keeps the leading digits that carry the shared part of a phone number or
/// CEP (the DDD area code, the CEP region) and randomizes the rest,
/// digit-by-digit, leaving punctuation exactly where it was. A value
/// containing any letter, or no digit at all, has no phone or CEP shape to
/// preserve a prefix of — it is free text wearing this column's name, and
/// every letter of it would otherwise survive verbatim — so it falls through
/// to [`redact`] instead, at every strength. `text` is the sanitized form of
/// the cell `source`; redrawn digits that keep a run of `source` in its source
/// position are moved off it (see [`steer_digits_off_source`]).
fn preserve_prefix(source: &str, text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let digit_count = text.chars().filter(char::is_ascii_digit).count();
    if digit_count == 0 || text.chars().any(char::is_alphabetic) {
        return redact(text, strength);
    }
    let prefix_len = prefix_length(digit_count, strength);
    let mut seen_digits = 0usize;
    let drawn: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                let keep = seen_digits < prefix_len;
                seen_digits += 1;
                if keep { c } else { random_digit(rng) }
            } else {
                c
            }
        })
        .collect();
    steer_digits_off_source(source, &drawn)
}

/// `drawn`, with each digit at which [`crate::compliance::survival_in_place`]
/// finds `source` surviving moved to the next digit, until nothing survives
/// or every digit has had a turn. Only a digit the strategy generated can
/// agree there, so the kept prefix is never touched. Draws no randomness, so
/// a value that keeps nothing of its source is exactly the draw, at every
/// strength.
fn steer_digits_off_source(source: &str, drawn: &str) -> String {
    let mut chars: Vec<char> = drawn.chars().collect();
    let digits = chars.iter().filter(|c| c.is_ascii_digit()).count();
    for _ in 0..digits {
        let candidate: String = chars.iter().collect();
        let Some(offset) = crate::compliance::survival_in_place(
            source,
            &candidate,
            Some(Strategy::PreservePrefix),
        ) else {
            return candidate;
        };
        match chars.get_mut(offset) {
            Some(c) if c.is_ascii_digit() => *c = next_digit(*c),
            _ => return candidate,
        }
    }
    chars.into_iter().collect()
}

fn next_digit(c: char) -> char {
    c.to_digit(10)
        .map_or(c, |digit| char::from(b'0' + ((digit + 1) % 10) as u8))
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
/// [`crate::classify::document_check_digits`] rather than reimplementing the
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

/// Regenerates a document for an integer-stored column classified
/// `semantic_type`: a CNPJ for [`SemanticType::Cnpj`], a CPF otherwise. The
/// type, not the source value, decides, because integer storage drops leading
/// zeros and so a short value could be either. The result may itself start
/// with a zero and so occupy fewer digits, exactly as a source document with a
/// leading zero does.
fn regenerate_integer_document(semantic_type: SemanticType, rng: &mut ChaCha8Rng) -> i64 {
    let digits = if semantic_type == SemanticType::Cnpj {
        cnpj_digits(rng)
    } else {
        cpf_digits(rng)
    };
    digits.iter().fold(0i64, |acc, &d| acc * 10 + i64::from(d))
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
    let check_digits = crate::classify::document_check_digits(&base, wrap);
    let mut digits = base;
    digits.extend(check_digits);
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
///
/// Every strength draws from the full corpus. Above [`Strength::Default`] the
/// drawn name never equals `text` case-insensitively: a draw landing on the
/// source moves the last token to the next corpus entry. The guard covers the draw;
/// [`crate::anomaly::reinject`], which runs afterwards, writes a value the leakage
/// check would flag for the row only when the anomaly's kind has no other shape to
/// fall back on, so a re-injected defect does not undo it.
// `Default` may land on the source: the leakage check flags it and escalation repairs it.
fn resample_name(text: &str, strength: Strength, rng: &mut ChaCha8Rng) -> String {
    let token_count = text.split_whitespace().count().max(1);
    let pool_for = |index: usize| if index == 0 { FIRST_NAMES } else { SURNAMES };
    let mut picks: Vec<usize> = (0..token_count)
        .map(|index| rng.random_range(0..pool_for(index).len()))
        .collect();
    let render = |picks: &[usize]| {
        picks
            .iter()
            .enumerate()
            .map(|(index, pick)| pool_for(index)[*pick])
            .collect::<Vec<_>>()
            .join(" ")
    };
    let name = render(&picks);
    if strength != Strength::Default && name.to_lowercase() == text.to_lowercase() {
        let last = token_count - 1;
        picks[last] = (picks[last] + 1) % pool_for(last).len();
        return render(&picks);
    }
    name
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

    fn text_key_spec(name: &str, pk: bool, fk: Option<ForeignKey>) -> ColumnSpec {
        ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Text,
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

    /// `resolve` must catch a `PreservePrefix` override on an integer
    /// column itself, before any row runs — not let it resolve successfully
    /// only to fail once the transform stage reaches a row.
    #[test]
    fn resolve_rejects_preserve_prefix_overriding_an_integer_column() {
        let mut overrides = BTreeMap::new();
        overrides.insert("orders.total_cents".to_owned(), Strategy::PreservePrefix);
        let spec = key_spec("total_cents", false, None);
        let error = resolve(
            &overrides,
            "orders.total_cents",
            &spec,
            SemanticType::NumericId,
        )
        .expect_err("rejected");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    /// A key column whose values are UUID-shaped mints through the
    /// dataset-wide UUID space instead of its class, even though it is a
    /// key — global identity for a UUID wins over the class-scoped default.
    #[test]
    fn resolve_sends_a_uuid_classified_key_column_to_mint_uuid() {
        let overrides = BTreeMap::new();
        let spec = text_key_spec("id", true, None);
        let strategy = resolve(&overrides, "t.id", &spec, SemanticType::Uuid).expect("resolves");
        assert_eq!(strategy, Strategy::MintUuid);
    }

    /// A non-key column classified `Uuid` — the audit/log table case with no
    /// declared foreign key — also mints through the dataset-wide UUID
    /// space, not through [`Strategy::Redact`]'s free-text fallback.
    #[test]
    fn resolve_sends_a_uuid_classified_non_key_column_to_mint_uuid() {
        let overrides = BTreeMap::new();
        let spec = plain_spec("entity_uuid");
        let strategy =
            resolve(&overrides, "t.entity_uuid", &spec, SemanticType::Uuid).expect("resolves");
        assert_eq!(strategy, Strategy::MintUuid);
    }

    #[test]
    fn email_keeps_the_domain_verbatim_and_changes_the_local_part() {
        let mut r = rng("email");
        let out = pseudonymize_email("ana@gmail.com", "ana@gmail.com", Strength::Default, &mut r);
        let (local, domain) = out.split_once('@').expect("well-formed email");
        assert_eq!(domain, "gmail.com");
        assert_ne!(local, "ana");
    }

    #[test]
    fn email_with_no_at_sign_still_produces_a_well_formed_address() {
        let mut r = rng("email-malformed");
        let out = pseudonymize_email("not-an-email", "not-an-email", Strength::Default, &mut r);
        assert!(
            classify::SemanticType::Email.matches_text(&out),
            "`{out}` is not email-shaped"
        );
    }

    #[test]
    fn email_with_a_stray_at_sign_still_keeps_the_trailing_domain() {
        let mut r = rng("email-stray-at");
        let out = pseudonymize_email(
            "also@@broken.com",
            "also@@broken.com",
            Strength::Default,
            &mut r,
        );
        assert!(
            out.ends_with("@broken.com"),
            "`{out}` must keep the trailing domain"
        );
        assert!(classify::SemanticType::Email.matches_text(&out));
    }

    #[test]
    fn preserve_prefix_keeps_the_ddd_for_a_phone_number() {
        let mut r = rng("phone");
        let out = preserve_prefix(
            "(11) 98765-4321",
            "(11) 98765-4321",
            Strength::Default,
            &mut r,
        );
        assert!(out.starts_with("(11)"), "`{out}` must keep the DDD");
        assert_eq!(out.len(), "(11) 98765-4321".len());
    }

    /// A phone-column value with no digits at all has nothing for
    /// `preserve_prefix` to randomize, so it must be redacted rather than
    /// survive byte-for-byte.
    #[test]
    fn preserve_prefix_redacts_a_value_with_no_digits_at_all() {
        let mut r = rng("phone-no-digits");
        let out = preserve_prefix(
            "NAO TEM TELEFONE",
            "NAO TEM TELEFONE",
            Strength::Default,
            &mut r,
        );
        assert_ne!(out, "NAO TEM TELEFONE");
    }

    #[test]
    fn preserve_prefix_redacts_a_value_containing_letters_at_every_strength() {
        for strength in [Strength::Default, Strength::Aggressive, Strength::Maximum] {
            for text in ["NASCIDO EM 1972", "(11) 98765-4321 ramal 22"] {
                let mut r = rng("phone-letters");
                let out = preserve_prefix(text, text, strength, &mut r);
                assert!(
                    !out.chars()
                        .any(|c| c.is_alphabetic() && c != 'x' && c != 'X')
                        || out == "REDACTED",
                    "`{out}` keeps letters of `{text}` at {strength:?}"
                );
                assert_ne!(out, text);
            }
        }
    }
    #[test]
    fn preserve_prefix_at_maximum_strength_keeps_no_digits() {
        let mut r = rng("cep-max");
        let out = preserve_prefix("01310-100", "01310-100", Strength::Maximum, &mut r);
        assert_ne!(out, "01310-100");
        assert_eq!(out.len(), "01310-100".len());
    }

    fn differing_positions(a: &str, b: &str) -> Vec<usize> {
        a.chars()
            .zip(b.chars())
            .enumerate()
            .filter(|(_, (x, y))| x != y)
            .map(|(index, _)| index)
            .collect()
    }

    #[test]
    fn a_drawn_local_part_keeping_a_short_word_in_place_is_moved_off_it() {
        let source = "joao.lima@x.com";
        let out = steer_local_part_off_source(source, "joaoab1234", "x.com");
        assert_eq!(differing_positions(&out, "joaoab1234@x.com"), vec![0]);
        assert!(out.ends_with("@x.com"));
        assert_eq!(
            crate::compliance::survival_in_place(
                source,
                &out,
                Some(Strategy::PseudonymizeLocalPart)
            ),
            None
        );
    }

    #[test]
    fn redrawn_phone_digits_equal_to_the_source_are_moved_off_it() {
        let source = "11987654321";
        let out = steer_digits_off_source(source, source);
        assert!(out.starts_with("11"), "`{out}` must keep the DDD");
        assert_eq!(differing_positions(&out, source).len(), 1);
        assert_eq!(
            crate::compliance::survival_in_place(source, &out, Some(Strategy::PreservePrefix)),
            None
        );
    }

    /// Fifteen digits keep a five-digit prefix and compare ten: one moved
    /// digit still leaves nine in ten agreeing, so a second is moved.
    #[test]
    fn a_fifteen_digit_run_agreeing_on_every_compared_digit_needs_two_moves() {
        let source = "123456789012345";
        let out = steer_digits_off_source(source, source);
        assert!(out.starts_with("12345"), "`{out}` must keep the prefix");
        assert_eq!(differing_positions(&out, source).len(), 2);
        assert_eq!(
            crate::compliance::survival_in_place(source, &out, Some(Strategy::PreservePrefix)),
            None
        );
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

    #[test]
    fn regenerate_integer_document_follows_the_semantic_type() {
        let mut r = rng("integer-document");
        for _ in 0..20 {
            let cpf = regenerate_integer_document(SemanticType::Cpf, &mut r);
            assert!(crate::classify::is_valid_cpf(&format!("{cpf:011}")));
            let cnpj = regenerate_integer_document(SemanticType::Cnpj, &mut r);
            assert!(crate::classify::is_valid_cnpj(&format!("{cnpj:014}")));
        }
    }

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
    fn an_escalated_resample_never_returns_a_corpus_source_as_itself() {
        let sources = [
            "Ana Silva",
            "Tiago Ribeiro",
            "Madonna",
            "Ana Maria Silva",
            "ANA SILVA",
        ];
        for strength in [Strength::Aggressive, Strength::Maximum] {
            for seed in 0..2_000 {
                let mut r = crate::rng::derive(seed, b"escalated-resample");
                for source in sources {
                    let out = resample_name(source, strength, &mut r);
                    assert_ne!(out.to_lowercase(), source.to_lowercase());
                    assert_eq!(
                        out.split_whitespace().count(),
                        source.split_whitespace().count()
                    );
                }
            }
        }
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

    /// A value made entirely of self-masking characters — all zeros, a run
    /// of the same letter, or punctuation alone — must still change under
    /// default-strength redaction. A single character is the degenerate
    /// case of each of those.
    #[test]
    fn redact_default_always_changes_a_nonempty_value() {
        let cases = [
            "000.000.000-0",
            "0000000000",
            "xxxxxxxxxx",
            "XXXXXXXXXX",
            "----------",
            "0",
            "x",
            "X",
            "-",
            ".",
        ];
        for input in cases {
            let out = redact(input, Strength::Default);
            assert_ne!(out, input, "`{input}` must change under default redaction");
            assert_eq!(
                out.chars().count(),
                input.chars().count(),
                "`{input}` must keep its length"
            );
        }
    }

    /// Redaction must actually break the document shape it masks, not
    /// merely differ byte-for-byte — flipping only the first character of
    /// an all-zero CPF would leave thirteen of fourteen characters
    /// identical to the source and still carry valid check digits.
    #[test]
    fn redact_default_of_an_all_zero_cpf_is_not_still_cpf_shaped() {
        let out = redact("000.000.000-00", Strength::Default);
        assert!(
            !classify::is_valid_cpf(&out),
            "`{out}` is still a valid CPF"
        );
    }

    #[test]
    fn redact_default_changes_every_character_of_a_caseless_script_value() {
        let input = "日本語";
        let out = redact(input, Strength::Default);
        assert_eq!(out.chars().count(), input.chars().count());
        for (original, masked) in input.chars().zip(out.chars()) {
            assert_ne!(masked, original, "`{original}` survived masking verbatim");
        }
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
        let ledger = Ledger {
            mappings,
            ..Default::default()
        };

        let mut r = rng("mint-key");
        let out = Strategy::MintKey
            .apply(
                "t.id",
                &Value::Int(1),
                Strength::Default,
                ColumnRole {
                    semantic_type: SemanticType::Unknown,
                    class: Some(class),
                },
                &ledger,
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
                ColumnRole {
                    semantic_type: SemanticType::Unknown,
                    class: None,
                },
                &ledger,
                &mut r,
            )
            .expect_err("no class means no lookup is possible");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    #[test]
    fn mint_uuid_looks_up_the_ledger_and_returns_the_mapped_value() {
        let mut uuid_mappings = BTreeMap::new();
        uuid_mappings.insert(
            "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(),
        );
        let ledger = Ledger {
            uuid_mappings,
            ..Default::default()
        };

        let mut r = rng("mint-uuid");
        let out = Strategy::MintUuid
            .apply(
                "audit_log.entity_uuid",
                &Value::Text("550e8400-e29b-41d4-a716-446655440000".to_owned()),
                Strength::Default,
                ColumnRole {
                    semantic_type: SemanticType::Unknown,
                    class: None,
                },
                &ledger,
                &mut r,
            )
            .expect("lookup succeeds");
        assert_eq!(
            out,
            Value::Text("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned())
        );
    }

    /// A value absent from the UUID ledger — never observed because it was
    /// not UUID-shaped — must redact rather than error, so a
    /// malformed entry in a `MintUuid` column cannot survive into the
    /// output verbatim.
    #[test]
    fn mint_uuid_on_a_value_absent_from_the_ledger_falls_back_to_redaction() {
        let ledger = Ledger::default();
        let mut r = rng("mint-uuid-missing");
        let out = Strategy::MintUuid
            .apply(
                "audit_log.entity_uuid",
                &Value::Text("joao.silva@example.com".to_owned()),
                Strength::Default,
                ColumnRole {
                    semantic_type: SemanticType::Unknown,
                    class: None,
                },
                &ledger,
                &mut r,
            )
            .expect("falls back to redaction rather than erroring");
        assert_ne!(out, Value::Text("joao.silva@example.com".to_owned()));
    }

    #[test]
    fn mint_uuid_on_a_non_text_value_is_an_incompatible_strategy_error() {
        let ledger = Ledger::default();
        let mut r = rng("mint-uuid-non-text");
        let error = Strategy::MintUuid
            .apply(
                "audit_log.entity_uuid",
                &Value::Int(1),
                Strength::Default,
                ColumnRole {
                    semantic_type: SemanticType::Unknown,
                    class: None,
                },
                &ledger,
                &mut r,
            )
            .expect_err("not a text value");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    fn apply_mint_handle(value: &Value, ledger: &Ledger) -> Result<Value, Error> {
        Strategy::MintHandle.apply(
            "users.login",
            value,
            Strength::Default,
            ColumnRole {
                semantic_type: SemanticType::Username,
                class: None,
            },
            ledger,
            &mut rng("mint-handle"),
        )
    }

    #[test]
    fn mint_handle_looks_up_the_ledger_and_returns_the_mapped_value() {
        let mut handle_mappings = BTreeMap::new();
        handle_mappings.insert("bob".to_owned(), "k3x9wq0a7".to_owned());
        let ledger = Ledger {
            handle_mappings,
            ..Default::default()
        };

        let out = apply_mint_handle(&Value::Text("bob".to_owned()), &ledger);
        assert_eq!(
            out.expect("lookup succeeds"),
            Value::Text("k3x9wq0a7".to_owned())
        );
        let null = apply_mint_handle(&Value::Null, &ledger);
        assert_eq!(null.expect("null is always representable"), Value::Null);
    }

    #[test]
    fn mint_handle_on_a_value_absent_from_the_ledger_is_an_incompatible_strategy_error() {
        let error = apply_mint_handle(&Value::Text("bob".to_owned()), &Ledger::default())
            .expect_err("an unobserved value has no handle");
        assert!(matches!(
            error,
            Error::IncompatibleStrategy {
                strategy: Strategy::MintHandle,
                ..
            }
        ));
    }

    #[test]
    fn mint_handle_on_a_non_text_value_is_an_incompatible_strategy_error() {
        let error =
            apply_mint_handle(&Value::Int(1), &Ledger::default()).expect_err("not a text value");
        assert!(matches!(error, Error::IncompatibleStrategy { .. }));
    }

    #[test]
    fn mint_handle_runs_on_text_columns_only() {
        assert!(type_compatible(Strategy::MintHandle, ColumnType::Text));
        for ty in [ColumnType::Integer, ColumnType::Float, ColumnType::Boolean] {
            assert!(!type_compatible(Strategy::MintHandle, ty));
        }
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
            Strategy::MintUuid,
        ] {
            let out = strategy
                .apply(
                    "t.x",
                    &Value::Null,
                    Strength::Default,
                    ColumnRole {
                        semantic_type: SemanticType::Unknown,
                        class: None,
                    },
                    &ledger,
                    &mut r,
                )
                .expect("null is always representable");
            assert_eq!(out, Value::Null);
        }
    }

    fn int_spec(name: &str) -> ColumnSpec {
        ColumnSpec {
            ty: ColumnType::Integer,
            nullable: true,
            ..plain_spec(name)
        }
    }

    fn repeated_texts(distinct: usize, each: usize) -> Vec<Value> {
        (0..distinct * each)
            .map(|i| Value::Text(format!("value{}", i % distinct)))
            .collect()
    }

    #[test]
    fn a_fold_reflects_a_value_back_across_the_bound_it_crossed() {
        assert_eq!(fold_int(110, 0, 100), 90);
        assert_eq!(fold_int(-5, 0, 100), 5);
        assert_eq!(fold_int(50, 0, 100), 50);
        assert_eq!(
            fold_int(1_000, 0, 100),
            0,
            "an overshooting fold is clamped"
        );
        assert_eq!(fold_int(i64::MAX, i64::MIN, 0), -i64::MAX);
        assert_eq!(fold_int(i64::MAX, 0, 1), 0, "no fold overflows");
        assert_eq!(fold_float(10.5, 0.0, 10.0), 9.5);
        assert_eq!(fold_float(-9.6e-7, 0.0, 10.0), 9.6e-7);
        assert_eq!(fold_float(1.0e12, 0.0, 10.0), 0.0);
        assert_eq!(fold_float(f64::NAN, 1.0, 10.0), 1.0);
    }

    #[test]
    fn folding_keeps_an_integer_column_integer_and_inside_its_source_range() {
        let source = vec![Value::Int(1), Value::Null, Value::Int(5), Value::Int(3)];
        let mut values = vec![Value::Int(9), Value::Null, Value::Int(-1), Value::Int(3)];
        SourceRange::of(&source).fold_into(&mut values, &source);
        assert_eq!(
            values,
            vec![Value::Int(2), Value::Null, Value::Int(3), Value::Int(3)]
        );
    }

    #[test]
    fn a_fold_that_lands_on_its_own_source_moves_one_step_inside() {
        let source = vec![Value::Int(0), Value::Int(96), Value::Int(100)];
        let mut values = vec![Value::Int(0), Value::Int(104), Value::Int(100)];
        SourceRange::of(&source).fold_into(&mut values, &source);
        assert_eq!(values[1], Value::Int(95));
    }

    #[test]
    fn jitter_with_out_of_range_reinjection_never_leaves_the_source_range() {
        let mut source: Vec<Value> = (0..40).map(|i| Value::Int(100 + i)).collect();
        source.push(Value::Int(0));
        source.push(Value::Int(10_000));
        let column = crate::dataset::ColumnData {
            spec: int_spec("valor"),
            values: source.clone(),
        };
        let profile = crate::profile::profile_column(
            "t.valor",
            &column,
            classify::Classification {
                semantic_type: SemanticType::NumericId,
                confidence: 1.0,
            },
        );
        let scan = crate::anomaly::detect(&column, &profile);
        assert!(
            scan.positions
                .contains_key(&crate::anomaly::AnomalyKind::OutOfRange)
        );

        for seed in 0..50u64 {
            let ledger = Ledger::default();
            let role = ColumnRole {
                semantic_type: SemanticType::NumericId,
                class: None,
            };
            let mut values: Vec<Value> = source
                .iter()
                .enumerate()
                .map(|(row, value)| {
                    let mut r = crate::rng::derive_for_row_position(seed, "t.valor", row, 2);
                    Strategy::Jitter
                        .apply("t.valor", value, Strength::Maximum, role, &ledger, &mut r)
                        .expect("jitter runs on integers")
                })
                .collect();
            let mut r = crate::rng::derive(seed, b"t.valor");
            crate::anomaly::reinject(&mut values, &scan, None, &mut r, |_, _| false);
            crate::anomaly::reinject_out_of_range_from_tail(&mut values, &source, &scan, &mut r);
            SourceRange::of(&source).fold_into(&mut values, &source);
            for value in &values {
                let Value::Int(n) = value else {
                    panic!("an integer column produced {value:?}");
                };
                assert!((0..=10_000).contains(n), "seed {seed}: {n} left the range");
            }
        }
    }

    #[test]
    fn a_shuffle_keeps_the_multiset_and_every_null_in_place() {
        let mut values = repeated_texts(4, 5);
        values[3] = Value::Null;
        values[11] = Value::Null;
        let shuffled = shuffle_values(&values, &mut rng("shuffle"));
        assert_ne!(shuffled, values, "a seeded shuffle of 18 values moves some");
        for (before, after) in values.iter().zip(&shuffled) {
            assert_eq!(before.is_null(), after.is_null());
        }
        let sorted = |values: &[Value]| {
            let mut texts: Vec<String> = values
                .iter()
                .filter_map(|value| value.as_text().map(str::to_owned))
                .collect();
            texts.sort();
            texts
        };
        assert_eq!(sorted(&values), sorted(&shuffled));
    }

    #[test]
    fn a_shuffle_is_a_function_of_its_stream_alone() {
        let values = repeated_texts(5, 4);
        assert_eq!(
            shuffle_values(&values, &mut rng("same")),
            shuffle_values(&values, &mut rng("same"))
        );
    }

    #[test]
    fn a_low_cardinality_free_text_or_integer_column_qualifies_for_shuffle() {
        let spec = plain_spec("status");
        let values = repeated_texts(SHUFFLE_MAX_DISTINCT, SHUFFLE_MIN_OCCURRENCES);
        assert!(qualifies_for_shuffle(
            Strategy::Redact,
            &spec,
            false,
            SemanticType::FreeText,
            &values
        ));
        let ratings: Vec<Value> = (0..15).map(|i| Value::Int(i % 5 + 1)).collect();
        assert!(qualifies_for_shuffle(
            Strategy::Jitter,
            &int_spec("rating"),
            false,
            SemanticType::NumericId,
            &ratings
        ));
    }

    #[test]
    fn a_column_failing_a_value_guard_is_not_shuffled() {
        let spec = plain_spec("status");
        let too_many = repeated_texts(SHUFFLE_MAX_DISTINCT + 1, SHUFFLE_MIN_OCCURRENCES);
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &spec,
            false,
            SemanticType::FreeText,
            &too_many
        ));
        let mut one_rare = repeated_texts(3, 4);
        one_rare.push(Value::Text("rare".to_owned()));
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &spec,
            false,
            SemanticType::FreeText,
            &one_rare
        ));
        let all_null = vec![Value::Null; 6];
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &spec,
            false,
            SemanticType::Unknown,
            &all_null
        ));
    }

    #[test]
    fn a_single_valued_column_is_not_shuffled() {
        let constant = vec![Value::Text("ativo".to_owned()); 12];
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &plain_spec("situacao"),
            false,
            SemanticType::FreeText,
            &constant
        ));
    }

    #[test]
    fn a_column_named_for_a_person_is_not_shuffled_and_one_named_for_a_state_is() {
        let logins: Vec<Value> = ["ana.lima", "bruno.reis", "carla.dias"]
            .iter()
            .cycle()
            .take(12)
            .map(|login| Value::Text((*login).to_owned()))
            .collect();
        for name in ["created_by", "username", "vendedorId", "nome-atendente"] {
            assert!(
                !qualifies_for_shuffle(
                    Strategy::Redact,
                    &plain_spec(name),
                    false,
                    SemanticType::FreeText,
                    &logins
                ),
                "`{name}` was shuffled"
            );
        }
        for name in ["status", "created_at", "usage", "agency_kind"] {
            assert!(
                qualifies_for_shuffle(
                    Strategy::Redact,
                    &plain_spec(name),
                    false,
                    SemanticType::FreeText,
                    &logins
                ),
                "`{name}` was not shuffled"
            );
        }
    }

    #[test]
    fn name_tokens_split_on_separators_and_camel_case() {
        assert_eq!(name_tokens("createdBy"), vec!["created", "by"]);
        assert_eq!(name_tokens("USER-NAME"), vec!["user", "name"]);
        assert_eq!(name_tokens("cod_vendedor2"), vec!["cod", "vendedor2"]);
    }

    #[test]
    fn keys_identifiers_and_non_text_redactions_are_never_shuffled() {
        let values = repeated_texts(3, 4);
        let key = text_key_spec("code", true, None);
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &key,
            false,
            SemanticType::FreeText,
            &values
        ));
        for default in [Strategy::MintKey, Strategy::MintUuid, Strategy::MintHandle] {
            assert!(!qualifies_for_shuffle(
                default,
                &plain_spec("code"),
                false,
                SemanticType::FreeText,
                &values
            ));
        }
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &plain_spec("code"),
            true,
            SemanticType::FreeText,
            &values
        ));
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &plain_spec("name"),
            false,
            SemanticType::FullName,
            &values
        ));
        let flags: Vec<Value> = (0..12).map(|i| Value::Bool(i % 2 == 0)).collect();
        let flag_spec = ColumnSpec {
            ty: ColumnType::Boolean,
            ..plain_spec("active")
        };
        assert!(!qualifies_for_shuffle(
            Strategy::Redact,
            &flag_spec,
            false,
            SemanticType::Unknown,
            &flags
        ));
        let prices: Vec<Value> = (0..12).map(|i| Value::Float(f64::from(i % 3))).collect();
        let price_spec = ColumnSpec {
            ty: ColumnType::Float,
            ..plain_spec("price")
        };
        assert!(!qualifies_for_shuffle(
            Strategy::Jitter,
            &price_spec,
            false,
            SemanticType::NumericId,
            &prices
        ));
    }
}
