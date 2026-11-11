use std::collections::{BTreeMap, BTreeSet};

use rand::RngExt;
use rand_chacha::ChaCha8Rng;

use crate::classify::{self, SemanticType};
use crate::context::RunContext;
use crate::dataset::{ColumnData, Dataset, KeyValue, Value};
use crate::error::Error;

/// Index of a referential equivalence class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(pub usize);

/// Union-find over key columns joined by foreign key edges. A chain
/// `c.x -> b.y -> a.z` collapses to one class, which is what makes referential
/// integrity hold transitively and dissolves foreign key cycles without a
/// topological sort.
///
/// A composite primary key contributes one node per member column rather than
/// one node for the tuple.
// TODO: fine tune — correlating the members of a composite primary key with
// each other is out of scope for the PoC; each column of a composite key is
// unioned only through its own foreign key edges.
#[derive(Debug, Clone, Default)]
pub struct EquivalenceClasses {
    /// `table.column` to class, in sorted key order.
    pub members: BTreeMap<String, ClassId>,
    pub count: usize,
}

impl EquivalenceClasses {
    /// Assumes `dataset` has already passed schema validation: every table
    /// name is unique and every foreign key's target table and column exist.
    /// A target that exists but is not itself a key is schema validation's
    /// responsibility, not this function's — `build` only ever reports a
    /// target as dangling, never as present-but-not-a-key.
    pub fn build(dataset: &Dataset) -> Result<Self, Error> {
        let mut nodes = BTreeSet::new();
        for table in &dataset.tables {
            for column in &table.columns {
                if column.spec.is_key() {
                    nodes.insert(column.qualified_name(&table.name));
                }
            }
        }

        let names: Vec<String> = nodes.into_iter().collect();
        let index_of: BTreeMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(index, name)| (name.as_str(), index))
            .collect();

        let mut forest = Forest::new(names.len());
        for table in &dataset.tables {
            for column in &table.columns {
                let Some(fk) = &column.spec.fk else { continue };
                let source = column.qualified_name(&table.name);
                let target = format!("{}.{}", fk.table, fk.column);
                match (index_of.get(source.as_str()), index_of.get(target.as_str())) {
                    (Some(&a), Some(&b)) => forest.union(a, b),
                    _ => {
                        return Err(Error::DanglingForeignKey {
                            column: source,
                            target: fk.clone(),
                        });
                    }
                }
            }
        }

        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for index in 0..names.len() {
            groups.entry(forest.find(index)).or_default().push(index);
        }

        let mut ordered_groups: Vec<Vec<usize>> = groups.into_values().collect();
        ordered_groups.sort_by_key(|group| group.iter().min().copied().unwrap_or(usize::MAX));

        let mut members = BTreeMap::new();
        for (class_index, group) in ordered_groups.iter().enumerate() {
            for &node_index in group {
                members.insert(names[node_index].clone(), ClassId(class_index));
            }
        }

        Ok(Self {
            count: ordered_groups.len(),
            members,
        })
    }

    pub fn class_of(&self, qualified_column: &str) -> Option<ClassId> {
        self.members.get(qualified_column).copied()
    }
}

/// Disjoint-set forest with path compression and union by size.
#[derive(Debug)]
struct Forest {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl Forest {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            size: vec![1; len],
        }
    }

    fn find(&mut self, node: usize) -> usize {
        let mut root = node;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = node;
        while self.parent[current] != root {
            let next = self.parent[current];
            self.parent[current] = root;
            current = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a == root_b {
            return;
        }
        if self.size[root_a] < self.size[root_b] {
            self.parent[root_a] = root_b;
            self.size[root_b] += self.size[root_a];
        } else {
            self.parent[root_b] = root_a;
            self.size[root_a] += self.size[root_b];
        }
    }
}

/// The document every key of a class is, when the class's minted members are
/// classified a CPF or a CNPJ. A class holding one mints each key with its
/// source's check-digit validity, so a valid document mints a valid document
/// and a malformed one a malformed one, in the same shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum KeyDocument {
    Cpf,
    Cnpj,
}

impl KeyDocument {
    pub(crate) fn for_semantic_type(semantic_type: SemanticType) -> Option<Self> {
        match semantic_type {
            SemanticType::Cpf => Some(Self::Cpf),
            SemanticType::Cnpj => Some(Self::Cnpj),
            _ => None,
        }
    }

    pub(crate) fn semantic_type(self) -> SemanticType {
        match self {
            Self::Cpf => SemanticType::Cpf,
            Self::Cnpj => SemanticType::Cnpj,
        }
    }

    /// Digits the two check digits are computed over.
    fn body_len(self) -> usize {
        match self {
            Self::Cpf => 9,
            Self::Cnpj => 12,
        }
    }

    /// Whether the check-digit weights fall back to 2 past 9.
    fn wraps(self) -> bool {
        match self {
            Self::Cpf => false,
            Self::Cnpj => true,
        }
    }

    fn digit_count(self) -> usize {
        self.body_len() + 2
    }

    /// Whether `key` is a valid document of this type, by the same rule
    /// anomaly detection applies to the column it came from
    /// ([`classify::is_valid_document`]'s integer and text arms).
    fn is_valid(self, key: &KeyValue) -> bool {
        match key {
            KeyValue::Int(n) => classify::is_valid_integer_document(self.semantic_type(), *n),
            KeyValue::Text(text) => classify::is_valid_text_document(self.semantic_type(), text),
        }
    }

    /// Overwrites `candidate`'s last two digits with the check digits of the
    /// digits before them, keeping its width and every non-digit character in
    /// place. An integer is read zero-padded to [`Self::digit_count`] digits,
    /// as integer storage drops a document's leading zeros. A candidate that
    /// does not hold exactly that many digits, or is negative, is returned
    /// unchanged and fails [`Self::is_valid`].
    fn complete_check_digits(self, candidate: KeyValue) -> KeyValue {
        match candidate {
            KeyValue::Int(n) if n >= 0 => {
                let mut body = vec![0u32; self.body_len()];
                let mut rest = n / 100;
                for digit in body.iter_mut().rev() {
                    *digit = (rest % 10) as u32;
                    rest /= 10;
                }
                let [first, second] = classify::document_check_digits(&body, self.wraps());
                KeyValue::Int(n - n % 100 + 10 * i64::from(first) + i64::from(second))
            }
            KeyValue::Text(text) => {
                let mut chars: Vec<char> = text.chars().collect();
                let digit_positions: Vec<usize> = chars
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.is_ascii_digit())
                    .map(|(index, _)| index)
                    .collect();
                if digit_positions.len() != self.digit_count() {
                    return KeyValue::Text(text);
                }
                let (body_positions, check_positions) = digit_positions.split_at(self.body_len());
                let body: Vec<u32> = body_positions
                    .iter()
                    .filter_map(|&index| chars[index].to_digit(10))
                    .collect();
                let check_digits = classify::document_check_digits(&body, self.wraps());
                for (&index, digit) in check_positions.iter().zip(check_digits) {
                    chars[index] = char::from(b'0' + digit as u8);
                }
                KeyValue::Text(chars.into_iter().collect())
            }
            negative => negative,
        }
    }
}

/// Collects the observed key domain of every class, plus the dataset-wide
/// domains of UUID-shaped values and of handle-column values, which mint
/// outside the class system entirely. Minting is insertion-order dependent,
/// so this runs single-threaded in a deterministic order: classes sorted by
/// name, values in row order; the UUID and handle domains in
/// qualified-column-name order, values in row order within a column.
#[derive(Debug, Default)]
pub struct LedgerBuilder {
    pub classes: EquivalenceClasses,
    pub domains: BTreeMap<ClassId, Vec<KeyValue>>,
    pub uuid_domain: Vec<String>,
    pub handle_domain: Vec<String>,
    /// The document each class's keys are, for a class that holds one; a
    /// class absent here mints without regard to check digits.
    pub(crate) documents: BTreeMap<ClassId, KeyDocument>,
    /// Membership check for [`Self::observe`]'s dedup: never iterated, so its
    /// hashing order cannot leak into the deterministic domain order that
    /// `domains` preserves.
    seen: BTreeMap<ClassId, BTreeSet<KeyValue>>,
    uuid_seen: BTreeSet<String>,
    handle_seen: BTreeSet<String>,
}

impl LedgerBuilder {
    pub fn new(classes: EquivalenceClasses) -> Self {
        Self {
            classes,
            domains: BTreeMap::new(),
            uuid_domain: Vec::new(),
            handle_domain: Vec::new(),
            documents: BTreeMap::new(),
            seen: BTreeMap::new(),
            uuid_seen: BTreeSet::new(),
            handle_seen: BTreeSet::new(),
        }
    }

    /// Records one occurrence of `value` in `class`'s domain. Idempotent: a
    /// value already seen in this class is not pushed again, so a caller can
    /// call this once per row without pre-deduplicating, and the first
    /// occurrence in scan order is what fixes the value's place in the
    /// domain.
    pub fn observe(&mut self, class: ClassId, value: &KeyValue) {
        if self.seen.entry(class).or_default().insert(value.clone()) {
            self.domains.entry(class).or_default().push(value.clone());
        }
    }

    /// Records one occurrence of `text` in the dataset-wide UUID domain,
    /// idempotent exactly as [`Self::observe`] is for a class's domain.
    pub fn observe_uuid(&mut self, text: &str) {
        if self.uuid_seen.insert(text.to_owned()) {
            self.uuid_domain.push(text.to_owned());
        }
    }

    /// Records one occurrence of `text` in the dataset-wide handle domain,
    /// idempotent exactly as [`Self::observe`] is for a class's domain.
    pub fn observe_handle(&mut self, text: &str) {
        if self.handle_seen.insert(text.to_owned()) {
            self.handle_domain.push(text.to_owned());
        }
    }

    /// [`Self::observe_handle`]s every text value of every column in
    /// `handle_columns`, in qualified-name order, values in row order.
    /// `handle_columns` is the set of columns
    /// [`crate::transform::Strategy::MintHandle`] was resolved for, never a
    /// key column. Every value is observed whatever its shape, so every
    /// lookup the transform makes finds a handle and no value of such a
    /// column falls back to anything that derives from its source.
    pub fn observe_handle_columns(&mut self, dataset: &Dataset, handle_columns: &BTreeSet<String>) {
        for qualified in handle_columns {
            let Some(column) = resolve_column(dataset, qualified) else {
                continue;
            };
            for value in &column.values {
                if let Value::Text(text) = value {
                    self.observe_handle(text);
                }
            }
        }
    }

    /// Walks every key column of `dataset` not in `uuid_columns` or
    /// `preserved` and [`Self::observe`]s its values into its class's
    /// domain, then walks every column named in `uuid_columns` — key or
    /// not, minus `preserved` — and [`Self::observe_uuid`]s its values into
    /// the one dataset-wide domain, in qualified-name order. `uuid_columns`
    /// is the set of columns [`crate::transform::Strategy::MintUuid`] was
    /// resolved for: a column routed there mints through the global UUID
    /// space instead of its class's, declared foreign key or not, which is
    /// what lets two unrelated tables' UUID columns agree on the same
    /// value without one. A UUID-shaped column's non-UUID-shaped values —
    /// a stray malformed entry in an otherwise UUID column — are never
    /// observed at all: [`crate::transform::Strategy::apply`] has no ledger
    /// entry to mint them against and falls back to redaction instead,
    /// which is what keeps a non-UUID value out of the global identity
    /// space it does not belong in. `preserved` is the set of columns
    /// resolved to [`crate::transform::Strategy::Keep`]: a preserved
    /// column's values never get minted, so observing them would only
    /// spend domain slots the run never uses.
    ///
    /// An FK value with no matching parent row is observed exactly like any
    /// other: it has no way to be distinguished from a valid reference at
    /// this point, and it must still be minted so the orphan survives into
    /// the output rather than vanishing or merging with an unrelated row.
    pub fn observe_dataset(
        &mut self,
        dataset: &Dataset,
        uuid_columns: &BTreeSet<String>,
        preserved: &BTreeSet<String>,
    ) {
        let mut columns_by_class: BTreeMap<ClassId, Vec<String>> = BTreeMap::new();
        for (qualified, &class) in &self.classes.members {
            if uuid_columns.contains(qualified) || preserved.contains(qualified) {
                continue;
            }
            columns_by_class
                .entry(class)
                .or_default()
                .push(qualified.clone());
        }

        for class_index in 0..self.classes.count {
            let class = ClassId(class_index);
            let Some(columns) = columns_by_class.get(&class) else {
                continue;
            };
            for qualified in columns {
                let Some(column) = resolve_column(dataset, qualified) else {
                    continue;
                };
                for value in &column.values {
                    if let Some(key) = KeyValue::from_value(value) {
                        self.observe(class, &key);
                    }
                }
            }
        }

        for qualified in uuid_columns {
            if preserved.contains(qualified) {
                continue;
            }
            let Some(column) = resolve_column(dataset, qualified) else {
                continue;
            };
            for value in &column.values {
                if let Value::Text(text) = value {
                    if classify::is_uuid_shaped(text) {
                        self.observe_uuid(text);
                    }
                }
            }
        }
    }

    /// Mints one injective mapping per class and one injective mapping each
    /// over the dataset-wide UUID and handle domains, then freezes. The
    /// parallel transform only ever sees `&Ledger`.
    pub fn freeze(self, context: &RunContext) -> Result<Ledger, Error> {
        let no_sources = BTreeSet::new();
        let mut mappings = BTreeMap::new();
        for class_index in 0..self.classes.count {
            let class = ClassId(class_index);
            let Some(domain) = self.domains.get(&class) else {
                continue;
            };
            let document = self.documents.get(&class).copied();
            let class_sources = self.seen.get(&class).unwrap_or(&no_sources);
            let mut used: BTreeSet<KeyValue> = BTreeSet::new();
            let mut mapping = BTreeMap::new();
            for source in domain {
                let target = mint(context, class, source, &used, document, class_sources)?;
                used.insert(target.clone());
                mapping.insert(source.clone(), target);
            }
            mappings.insert(class, mapping);
        }

        let mut uuid_used: BTreeSet<String> = BTreeSet::new();
        let mut uuid_mappings = BTreeMap::new();
        for source in &self.uuid_domain {
            let target = mint_uuid(context, source, &uuid_used)?;
            uuid_used.insert(target.clone());
            uuid_mappings.insert(source.clone(), target);
        }

        let handle_mappings = mint_handle_mappings(context, &self.handle_domain, handle_len)?;

        Ok(Ledger {
            mappings,
            uuid_mappings,
            handle_mappings,
        })
    }
}

fn resolve_column<'a>(dataset: &'a Dataset, qualified: &str) -> Option<&'a ColumnData> {
    let (table_name, column_name) = qualified.split_once('.')?;
    dataset.table(table_name)?.column(column_name)
}

/// Frozen, injective key mapping, in three parts: a per-class mapping for
/// ordinary integer and text keys, one dataset-wide mapping for UUID-shaped
/// text, and one dataset-wide mapping for the values of handle columns. The
/// latter two mint outside the class system so that the same UUID or handle
/// anywhere in the dataset agrees on the same output regardless of which
/// column observed it. Keys are minted rather than hashed:
/// hashing a six-figure key domain into a fixed width collides, and a
/// collision silently merges two parent rows.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub mappings: BTreeMap<ClassId, BTreeMap<KeyValue, KeyValue>>,
    pub uuid_mappings: BTreeMap<String, String>,
    pub handle_mappings: BTreeMap<String, String>,
}

impl Ledger {
    pub fn lookup(&self, class: ClassId, value: &KeyValue) -> Option<&KeyValue> {
        self.mappings.get(&class)?.get(value)
    }

    pub fn lookup_uuid(&self, text: &str) -> Option<&str> {
        self.uuid_mappings.get(text).map(String::as_str)
    }

    pub fn lookup_handle(&self, text: &str) -> Option<&str> {
        self.handle_mappings.get(text).map(String::as_str)
    }
}

/// Mints `source`'s counterpart within `class`: a candidate of the same
/// [`KeyValue`] variant and the same decimal width or character length,
/// drawn from an RNG keyed on the class and the source value itself (never
/// on row position — the same source value must land on the same minted
/// value everywhere it recurs, which is what makes requirement #4 hold).
/// Collisions against `used` are resolved by drawing again from the same
/// stream, never by truncating or falling back to a hash, up to a cap sized
/// to the candidate space so a genuinely exhausted domain fails loudly
/// instead of spinning.
///
/// In a class whose keys are a [`KeyDocument`], the candidate also keeps
/// `source`'s check-digit validity. A valid source mints a candidate whose
/// last two digits are completed into its check digits, and avoids every
/// value of `class_sources` (the class's whole observed domain) as long as
/// its cell leaves room to. A malformed source mints a candidate that is
/// not a valid document. Candidates fall into disjoint cells by sign, width
/// and validity, and each cell holds exactly the values whose sources can
/// land in it, so an injective mapping always exists whenever one did
/// without the document rule.
fn mint(
    context: &RunContext,
    class: ClassId,
    source: &KeyValue,
    used: &BTreeSet<KeyValue>,
    document: Option<KeyDocument>,
    class_sources: &BTreeSet<KeyValue>,
) -> Result<KeyValue, Error> {
    let mut rng = context.rng_for(&mint_label(class, source));
    let plain = |rng: &mut ChaCha8Rng| mint_candidate(source, rng);
    let minted = match document {
        None => draw(
            &mut rng,
            mint_attempt_cap(candidate_domain_size(source)),
            plain,
            |candidate| !used.contains(candidate),
        ),
        Some(document) if document.is_valid(source) => {
            let cap = mint_attempt_cap(valid_document_domain_size(document, source));
            let completed =
                |rng: &mut ChaCha8Rng| document.complete_check_digits(mint_candidate(source, rng));
            draw(&mut rng, cap, completed, |candidate| {
                document.is_valid(candidate)
                    && !used.contains(candidate)
                    && !class_sources.contains(candidate)
            })
            .or_else(|| {
                draw(&mut rng, cap, completed, |candidate| {
                    document.is_valid(candidate) && !used.contains(candidate)
                })
            })
        }
        Some(document) => draw(
            &mut rng,
            mint_attempt_cap(candidate_domain_size(source)),
            plain,
            |candidate| !document.is_valid(candidate) && !used.contains(candidate),
        ),
    };
    minted.ok_or(Error::KeyDomainExhausted { class: class.0 })
}

/// Draws up to `cap` candidates from `rng` through `generate` and returns the
/// first one `accept` takes, consuming the stream only as far as that one.
fn draw(
    rng: &mut ChaCha8Rng,
    cap: usize,
    mut generate: impl FnMut(&mut ChaCha8Rng) -> KeyValue,
    accept: impl Fn(&KeyValue) -> bool,
) -> Option<KeyValue> {
    (0..cap)
        .map(|_| generate(rng))
        .find(|candidate| accept(candidate))
}

/// How many valid documents of `document`'s type share `source`'s shape: an
/// integer of decimal width `w` (a valid one is at least three digits wide)
/// fixes its leading digit nonzero and its last two digits as check digits,
/// leaving `9·10^(w-3)`; a text document fixes only its two check digits.
fn valid_document_domain_size(document: KeyDocument, source: &KeyValue) -> u128 {
    match source {
        KeyValue::Int(n) => 9u128.saturating_mul(10u128.pow(digit_width(*n).saturating_sub(3))),
        KeyValue::Text(_) => 10u128.pow(document.digit_count().saturating_sub(2) as u32),
    }
}

/// Distinguishes `Int` from `Text` sources in the same class and the same
/// class from another, so no two distinct `(class, source)` pairs can ever
/// compose to the same label.
fn mint_label(class: ClassId, source: &KeyValue) -> String {
    match source {
        KeyValue::Int(n) => format!("ledger.{}.int.{n}", class.0),
        KeyValue::Text(s) => format!("ledger.{}.text.{s}", class.0),
    }
}

fn mint_candidate(source: &KeyValue, rng: &mut ChaCha8Rng) -> KeyValue {
    match source {
        KeyValue::Int(n) => KeyValue::Int(mint_int(*n, rng)),
        KeyValue::Text(s) => KeyValue::Text(mint_text(s, rng)),
    }
}

/// Caps how many candidates [`mint`] draws before giving up on a class whose
/// candidate space is too small to hold every distinct value injectively.
/// Generous relative to the space itself, so a realistic run never comes
/// close, while still bounding the cost of a genuinely exhausted one.
fn mint_attempt_cap(domain_size: u128) -> usize {
    domain_size.saturating_mul(50).clamp(64, 5_000_000) as usize
}

fn candidate_domain_size(source: &KeyValue) -> u128 {
    match source {
        KeyValue::Int(n) => int_domain_size(digit_width(*n), *n < 0),
        KeyValue::Text(s) => text_domain_size(s),
    }
}

/// Number of decimal digits in `n`'s magnitude; zero has width one so it
/// stays representable in its own one-digit domain.
fn digit_width(n: i64) -> u32 {
    n.unsigned_abs().to_string().len() as u32
}

/// Inclusive bounds on the magnitude of an integer of decimal width `width`,
/// for a source of the given sign. Width 1 spans `0..=9` for a non-negative
/// source, so zero is representable, but `1..=9` for a negative one: zero
/// has no sign of its own, so drawing it for a negative source would mint a
/// non-negative value and silently hand that magnitude to both sign's
/// sub-domains at once. Clamped at `i64::MAX` for width 19 (the widest an
/// `i64` magnitude can be), since the mathematical upper bound `10^19 - 1`
/// does not fit in an `i64`.
fn int_bounds(width: u32, negative: bool) -> (i64, i64) {
    match width {
        0 | 1 if negative => (1, 9),
        0 | 1 => (0, 9),
        19.. => (10i64.pow(18), i64::MAX),
        _ => (10i64.pow(width - 1), 10i64.pow(width) - 1),
    }
}

fn int_domain_size(width: u32, negative: bool) -> u128 {
    let (low, high) = int_bounds(width, negative);
    (high - low + 1) as u128
}

/// Generates a candidate of the same sign and decimal width as `source` —
/// e.g. a three-digit source always mints another three-digit magnitude.
/// A negative source never draws a zero magnitude: `-0 == 0` would mint a
/// non-negative value and collide with the positive sub-domain's own zero.
fn mint_int(source: i64, rng: &mut ChaCha8Rng) -> i64 {
    let negative = source < 0;
    let (low, high) = int_bounds(digit_width(source), negative);
    let magnitude = rng.random_range(low..=high);
    if negative { -magnitude } else { magnitude }
}

/// Generates a candidate of the same length as `source`, redrawing each
/// digit and letter independently — onto a fixed ASCII class, preserving
/// case for a cased letter — while keeping every other character verbatim,
/// so punctuation that carries the key's visual shape (hyphens, slashes)
/// survives unchanged while every identifying character does not, whichever
/// script it was written in.
fn mint_text(source: &str, rng: &mut ChaCha8Rng) -> String {
    source.chars().map(|c| mint_char(c, rng)).collect()
}

/// True for any character minting redraws as a digit: an ASCII digit, or any
/// other Unicode decimal digit — a non-ASCII digit carries the same
/// identifying weight as an ASCII one and must not survive minting verbatim.
fn is_redrawable_digit(c: char) -> bool {
    c.is_numeric()
}

/// True for any character minting redraws as a letter, cased or not — a
/// non-ASCII letter (Cyrillic, CJK, …) is exactly what must not survive
/// minting verbatim.
fn is_redrawable_letter(c: char) -> bool {
    c.is_alphabetic()
}

fn mint_char(c: char, rng: &mut ChaCha8Rng) -> char {
    if is_redrawable_digit(c) {
        char::from(b'0' + rng.random_range(0..10u8))
    } else if is_redrawable_letter(c) {
        if c.is_lowercase() {
            char::from(b'a' + rng.random_range(0..26u8))
        } else {
            char::from(b'A' + rng.random_range(0..26u8))
        }
    } else {
        c
    }
}

fn text_domain_size(source: &str) -> u128 {
    source
        .chars()
        .fold(1u128, |acc, c| acc.saturating_mul(char_domain_size(c)))
}

fn char_domain_size(c: char) -> u128 {
    if is_redrawable_digit(c) {
        10
    } else if is_redrawable_letter(c) {
        26
    } else {
        1
    }
}

/// Mints `source`'s counterpart in the dataset-wide UUID identity space:
/// every hex digit (`0-9a-fA-F`) is redrawn within the full sixteen-value hex
/// alphabet, case preserved for a letter; every other character — the
/// hyphens, for the hyphenated form — survives verbatim. Keyed on `source`
/// alone, never on a class or row position: the whole point of this space is
/// that the same UUID anywhere in the dataset agrees on the same output
/// regardless of which class, if any, happened to observe it. Collisions
/// against `used` are redrawn from the same stream exactly as [`mint`]'s are;
/// a 32-hex-digit domain is astronomically larger than the attempt cap could
/// ever threaten, so this never spins in practice.
fn mint_uuid(context: &RunContext, source: &str, used: &BTreeSet<String>) -> Result<String, Error> {
    let cap = mint_attempt_cap(uuid_domain_size(source));
    let mut rng = context.rng_for(&format!("ledger.uuid.{source}"));
    for _ in 0..cap {
        let candidate = mint_uuid_text(source, &mut rng);
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::UuidDomainExhausted)
}

fn mint_uuid_text(source: &str, rng: &mut ChaCha8Rng) -> String {
    source.chars().map(|c| mint_hex_char(c, rng)).collect()
}

/// Redraws one hex-digit character within `0-9a-fA-F`, preserving case for a
/// letter; any other character (a hyphen, or stray non-hex content in a
/// malformed source) passes through unchanged. Digits and hex letters share
/// one sixteen-value draw rather than two disjoint ten- and six-value ones,
/// so a digit position can mint to a letter and vice versa — both are
/// equally valid hex digits, and narrowing the draw to the source
/// character's own sub-alphabet would needlessly shrink the candidate space.
fn mint_hex_char(c: char, rng: &mut ChaCha8Rng) -> char {
    if !c.is_ascii_hexdigit() {
        return c;
    }
    let value = rng.random_range(0..16u8);
    let lowercase = if value < 10 {
        b'0' + value
    } else {
        b'a' + (value - 10)
    };
    if c.is_ascii_uppercase() {
        char::from(lowercase).to_ascii_uppercase()
    } else {
        char::from(lowercase)
    }
}

fn uuid_domain_size(source: &str) -> u128 {
    source
        .chars()
        .fold(1u128, |acc, c| acc.saturating_mul(uuid_char_domain_size(c)))
}

fn uuid_char_domain_size(c: char) -> u128 {
    if c.is_ascii_hexdigit() { 16 } else { 1 }
}

/// Mints every source of `domain`, in order, to a handle `len` of its
/// character count long. A minted handle never equals any source of
/// `domain`, compared case-insensitively, nor a handle minted before it: the
/// leakage check compares a direct identifier's output whole against its
/// column's sources, and minting does not change under escalation, so a
/// coincidence would fail the run at every strength. `len` is
/// [`handle_len`] outside tests.
fn mint_handle_mappings(
    context: &RunContext,
    domain: &[String],
    len: fn(usize) -> usize,
) -> Result<BTreeMap<String, String>, Error> {
    let mut used: BTreeSet<String> = domain.iter().map(|source| source.to_lowercase()).collect();
    let mut mappings = BTreeMap::new();
    for source in domain {
        let target = mint_handle_at_len(context, source, len(source.chars().count()), &used)?;
        used.insert(target.clone());
        mappings.insert(source.clone(), target);
    }
    Ok(mappings)
}

/// Mints `source`'s counterpart in the dataset-wide handle space: a fresh
/// `len`-character handle, drawn from a stream keyed on `source` alone and
/// never built from its characters. Injectivity comes from redrawing on a
/// collision with `used`, up to a cap sized to the candidate space, so an
/// exhausted space fails loudly rather than letting two sources share a
/// handle.
fn mint_handle_at_len(
    context: &RunContext,
    source: &str,
    len: usize,
    used: &BTreeSet<String>,
) -> Result<String, Error> {
    let cap = mint_attempt_cap(handle_domain_size(len));
    let mut rng = context.rng_for(&format!("ledger.handle.{source}"));
    for _ in 0..cap {
        let candidate = mint_handle_text(len, &mut rng);
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::HandleDomainExhausted)
}

/// The source's own length, held to `8..=32`: the floor keeps a short
/// login's candidate space far larger than any realistic dataset, the cap
/// keeps a handle handle-sized. Independent of
/// [`crate::transform::Strength`], since minting happens once, before the
/// compliance retry loop.
fn handle_len(source_len: usize) -> usize {
    source_len.clamp(8, 32)
}

/// One handle candidate: a lowercase letter, then `len - 2` lowercase
/// letters or digits, then a digit.
fn mint_handle_text(len: usize, rng: &mut ChaCha8Rng) -> String {
    let mut handle = String::with_capacity(len);
    handle.push(mint_handle_letter(rng));
    for _ in 0..len.saturating_sub(2) {
        handle.push(mint_handle_alphanumeric(rng));
    }
    handle.push(mint_handle_digit(rng));
    handle
}

fn mint_handle_letter(rng: &mut ChaCha8Rng) -> char {
    char::from(b'a' + rng.random_range(0..26u8))
}

fn mint_handle_digit(rng: &mut ChaCha8Rng) -> char {
    char::from(b'0' + rng.random_range(0..10u8))
}

fn mint_handle_alphanumeric(rng: &mut ChaCha8Rng) -> char {
    let value = rng.random_range(0..36u8);
    if value < 26 {
        char::from(b'a' + value)
    } else {
        char::from(b'0' + (value - 26))
    }
}

/// `26 * 10` for the fixed first and last characters, times `36` for each of
/// the `len - 2` between them.
fn handle_domain_size(len: usize) -> u128 {
    let free_positions = len.saturating_sub(2) as u32;
    260u128.saturating_mul(36u128.saturating_pow(free_positions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, Table};

    fn pk_column(name: &str) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: true,
                fk: None,
            },
            values: Vec::new(),
        }
    }

    fn fk_column(name: &str, target_table: &str, target_column: &str) -> ColumnData {
        ColumnData {
            spec: ColumnSpec {
                name: name.to_owned(),
                ty: ColumnType::Integer,
                nullable: false,
                pk: false,
                fk: Some(ForeignKey {
                    table: target_table.to_owned(),
                    column: target_column.to_owned(),
                }),
            },
            values: Vec::new(),
        }
    }

    fn table(name: &str, columns: Vec<ColumnData>) -> Table {
        Table {
            name: name.to_owned(),
            columns,
        }
    }

    fn classes(dataset: &Dataset) -> EquivalenceClasses {
        match EquivalenceClasses::build(dataset) {
            Ok(classes) => classes,
            Err(error) => panic!("build failed: {error}"),
        }
    }

    #[test]
    fn chain_collapses_into_one_class() {
        let dataset = Dataset {
            tables: vec![
                table("a", vec![pk_column("z")]),
                table(
                    "b",
                    vec![{
                        let mut y = pk_column("y");
                        y.spec.fk = Some(ForeignKey {
                            table: "a".into(),
                            column: "z".into(),
                        });
                        y
                    }],
                ),
                table("c", vec![fk_column("x", "b", "y")]),
            ],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 1);
        assert_eq!(classes.class_of("a.z"), classes.class_of("b.y"));
        assert_eq!(classes.class_of("b.y"), classes.class_of("c.x"));
    }

    #[test]
    fn diamond_collapses_into_one_class() {
        let dataset = Dataset {
            tables: vec![
                table("a", vec![pk_column("id")]),
                table("b", vec![fk_column("a_id", "a", "id")]),
                table("c", vec![fk_column("a_id", "a", "id")]),
                table(
                    "d",
                    vec![
                        fk_column("b_id", "b", "a_id"),
                        fk_column("c_id", "c", "a_id"),
                    ],
                ),
            ],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 1);
        assert_eq!(classes.class_of("a.id"), classes.class_of("b.a_id"));
        assert_eq!(classes.class_of("a.id"), classes.class_of("c.a_id"));
        assert_eq!(classes.class_of("a.id"), classes.class_of("d.b_id"));
        assert_eq!(classes.class_of("a.id"), classes.class_of("d.c_id"));
    }

    #[test]
    fn mutual_foreign_keys_dissolve_the_cycle_into_one_class() {
        let dataset = Dataset {
            tables: vec![
                table(
                    "a",
                    vec![{
                        let mut id = pk_column("id");
                        id.spec.fk = Some(ForeignKey {
                            table: "b".into(),
                            column: "id".into(),
                        });
                        id
                    }],
                ),
                table(
                    "b",
                    vec![{
                        let mut id = pk_column("id");
                        id.spec.fk = Some(ForeignKey {
                            table: "a".into(),
                            column: "id".into(),
                        });
                        id
                    }],
                ),
            ],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 1);
        assert_eq!(classes.class_of("a.id"), classes.class_of("b.id"));
    }

    #[test]
    fn self_referencing_foreign_key_collapses_with_its_target() {
        let dataset = Dataset {
            tables: vec![table(
                "employees",
                vec![pk_column("id"), fk_column("manager_id", "employees", "id")],
            )],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 1);
        assert_eq!(
            classes.class_of("employees.id"),
            classes.class_of("employees.manager_id")
        );
    }

    #[test]
    fn isolated_keys_form_their_own_singleton_classes() {
        let dataset = Dataset {
            tables: vec![
                table("a", vec![pk_column("id")]),
                table("b", vec![pk_column("id")]),
            ],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 2);
        assert_ne!(classes.class_of("a.id"), classes.class_of("b.id"));
    }

    #[test]
    fn composite_primary_key_columns_are_independent_nodes() {
        let dataset = Dataset {
            tables: vec![table(
                "line_items",
                vec![pk_column("order_id"), pk_column("product_id")],
            )],
        };

        let classes = classes(&dataset);
        assert_eq!(classes.count, 2);
        assert_ne!(
            classes.class_of("line_items.order_id"),
            classes.class_of("line_items.product_id")
        );
    }

    #[test]
    fn class_ordering_is_lexicographic_and_stable_across_runs() {
        let dataset = Dataset {
            tables: vec![
                table("zeta", vec![pk_column("id")]),
                table("alpha", vec![pk_column("id")]),
                table("mid", vec![pk_column("id")]),
            ],
        };

        let first = classes(&dataset);
        assert_eq!(first.class_of("alpha.id"), Some(ClassId(0)));
        assert_eq!(first.class_of("mid.id"), Some(ClassId(1)));
        assert_eq!(first.class_of("zeta.id"), Some(ClassId(2)));

        let second = classes(&dataset);
        assert_eq!(first.members, second.members);
        assert_eq!(first.count, second.count);
    }

    use crate::dataset::Value;
    use crate::policy::Policy;

    fn context(seed: u64) -> RunContext {
        RunContext::new(Policy {
            seed: Some(seed),
            ..Default::default()
        })
    }

    fn int_pk_column(name: &str, values: &[i64]) -> ColumnData {
        let mut column = pk_column(name);
        column.values = values.iter().copied().map(Value::Int).collect();
        column
    }

    fn text_pk_column(name: &str, values: &[&str]) -> ColumnData {
        let mut column = pk_column(name);
        column.spec.ty = ColumnType::Text;
        column.values = values
            .iter()
            .map(|v| Value::Text((*v).to_owned()))
            .collect();
        column
    }

    fn frozen(dataset: &Dataset, seed: u64) -> (EquivalenceClasses, Ledger) {
        let classes = match EquivalenceClasses::build(dataset) {
            Ok(classes) => classes,
            Err(error) => panic!("build failed: {error}"),
        };
        let mut builder = LedgerBuilder::new(classes.clone());
        builder.observe_dataset(dataset, &BTreeSet::new(), &BTreeSet::new());
        let ledger = match builder.freeze(&context(seed)) {
            Ok(ledger) => ledger,
            Err(error) => panic!("freeze failed: {error}"),
        };
        (classes, ledger)
    }

    /// The class's domain exactly fills its own candidate space (ten
    /// single-digit magnitudes, ten distinct sources), so every source
    /// after the first few is forced to collide against an already-minted
    /// value before landing on the one slot still free. The mapping must
    /// still end up injective, and since domain size equals class size the
    /// minted values must be a permutation of the source domain.
    #[test]
    fn minting_is_injective_under_collision_pressure() {
        let dataset = Dataset {
            tables: vec![table(
                "t",
                vec![int_pk_column("id", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])],
            )],
        };
        let (classes, ledger) = frozen(&dataset, 1);
        let class = classes.class_of("t.id").expect("t.id is a key");
        let mapping = &ledger.mappings[&class];

        assert_eq!(mapping.len(), 10);
        let minted: BTreeSet<&KeyValue> = mapping.values().collect();
        assert_eq!(minted.len(), 10, "minted values must be pairwise distinct");
        for value in 0..10i64 {
            assert!(
                minted.contains(&KeyValue::Int(value)),
                "minted set must be a permutation of the full single-digit domain"
            );
        }
    }

    /// Several negative single-digit sources at once, not just `-4` alongside
    /// one of every other width: a single-digit source draws its magnitude
    /// from the same `0..=9` domain a positive single-digit source draws
    /// from, so this is the shape that exposes a sign flip (`-0 == 0`) if one
    /// slips back in.
    #[test]
    fn minted_integers_preserve_sign_and_decimal_width() {
        let sources = [
            -98765i64,
            -4,
            -1,
            -2,
            -3,
            -5,
            -6,
            -7,
            -8,
            -9,
            0,
            1,
            2,
            3,
            7,
            42,
            1234,
            987_654_321,
        ];
        for seed in 0..10u64 {
            let dataset = Dataset {
                tables: vec![table("t", vec![int_pk_column("id", &sources)])],
            };
            let (classes, ledger) = frozen(&dataset, seed);
            let class = classes.class_of("t.id").expect("t.id is a key");
            let mapping = &ledger.mappings[&class];

            for source in sources {
                let KeyValue::Int(minted) = mapping[&KeyValue::Int(source)] else {
                    panic!("an integer source must mint an integer");
                };
                assert_eq!(
                    minted.is_negative(),
                    source.is_negative(),
                    "seed {seed}: sign must be preserved for {source}, got {minted}"
                );
                assert_eq!(
                    digit_width(minted),
                    digit_width(source),
                    "seed {seed}: decimal width must be preserved for {source}"
                );
            }
        }
    }

    /// With sign-aware bounds, the positive sub-domain `0..=9` and the
    /// negative sub-domain `-9..=-1` are disjoint, so the full single-digit
    /// domain `-9..=9` (19 sources) has exactly 19 candidate targets and
    /// must mint without exhausting its attempt cap — a stray sign flip
    /// would hand one magnitude to both sub-domains at once and reach the
    /// cap almost certainly.
    #[test]
    fn full_single_digit_domain_mints_without_exhaustion() {
        let sources: Vec<i64> = (-9..=9).collect();
        for seed in 0..10u64 {
            let dataset = Dataset {
                tables: vec![table("t", vec![int_pk_column("id", &sources)])],
            };
            let (classes, ledger) = frozen(&dataset, seed);
            let class = classes.class_of("t.id").expect("t.id is a key");
            let mapping = &ledger.mappings[&class];

            assert_eq!(mapping.len(), sources.len());
            let minted: BTreeSet<&KeyValue> = mapping.values().collect();
            assert_eq!(
                minted.len(),
                sources.len(),
                "seed {seed}: minted values must be pairwise distinct"
            );
            for &source in &sources {
                let KeyValue::Int(target) = mapping[&KeyValue::Int(source)] else {
                    panic!("an integer source must mint an integer");
                };
                assert_eq!(
                    target.is_negative(),
                    source.is_negative(),
                    "seed {seed}: sign must be preserved for {source}"
                );
            }
        }
    }

    #[test]
    fn minted_text_preserves_length() {
        let sources = ["A1B2C3", "00042", "x-7", "", "lowercase-key-9"];
        let dataset = Dataset {
            tables: vec![table("t", vec![text_pk_column("id", &sources)])],
        };
        let (classes, ledger) = frozen(&dataset, 3);
        let class = classes.class_of("t.id").expect("t.id is a key");
        let mapping = &ledger.mappings[&class];

        for source in sources {
            let KeyValue::Text(minted) = &mapping[&KeyValue::Text(source.to_owned())] else {
                panic!("a text source must mint text");
            };
            assert_eq!(
                minted.chars().count(),
                source.chars().count(),
                "length must be preserved for `{source}`"
            );
        }
    }

    /// A key made entirely of non-ASCII letters must not mint as an
    /// identity mapping: digits and letters are redrawable in any script,
    /// not only ASCII. Length is still preserved, but the content must not
    /// be.
    #[test]
    fn minted_text_redraws_non_ascii_letters_and_digits() {
        let sources = ["中文合同编号", "ПРОЕКТ-2024"];
        let dataset = Dataset {
            tables: vec![table("t", vec![text_pk_column("id", &sources)])],
        };
        let (classes, ledger) = frozen(&dataset, 5);
        let class = classes.class_of("t.id").expect("t.id is a key");
        let mapping = &ledger.mappings[&class];

        for source in sources {
            let KeyValue::Text(minted) = &mapping[&KeyValue::Text(source.to_owned())] else {
                panic!("a text source must mint text");
            };
            assert_eq!(minted.chars().count(), source.chars().count());
            assert_ne!(
                minted, source,
                "`{source}` must not survive minting verbatim"
            );
        }
    }

    /// An FK value absent from its target's observed PK domain (an orphan)
    /// is still a member of the class's domain and must still be minted,
    /// injectively alongside every other member — requirement #3 says
    /// dangling foreign keys, the canonical production bug, must survive.
    #[test]
    fn an_orphan_fk_value_is_minted_like_any_other_class_member() {
        let dataset = Dataset {
            tables: vec![
                table("customers", vec![int_pk_column("id", &[1, 2, 3])]),
                table(
                    "orders",
                    vec![{
                        let mut customer_id = fk_column("customer_id", "customers", "id");
                        customer_id.values = vec![1, 2, 999].into_iter().map(Value::Int).collect();
                        customer_id
                    }],
                ),
            ],
        };
        let (classes, ledger) = frozen(&dataset, 4);
        let class = classes
            .class_of("customers.id")
            .expect("customers.id is a key");
        assert_eq!(class, classes.class_of("orders.customer_id").unwrap());

        let mapping = &ledger.mappings[&class];
        assert!(
            mapping.contains_key(&KeyValue::Int(999)),
            "the orphan value must be minted"
        );

        let minted: BTreeSet<&KeyValue> = mapping.values().collect();
        assert_eq!(
            minted.len(),
            mapping.len(),
            "minting must stay injective across orphan and non-orphan values alike"
        );
    }

    #[test]
    fn freezing_the_same_domain_at_the_same_seed_is_deterministic() {
        let dataset = Dataset {
            tables: vec![table("t", vec![int_pk_column("id", &[10, 20, 30, 40, 50])])],
        };
        let (_, first) = frozen(&dataset, 77);
        let (_, second) = frozen(&dataset, 77);
        assert_eq!(first.mappings, second.mappings);
    }

    /// A minted UUID must itself be well-formed and the same shape as its
    /// source: same length, same hyphen positions, and it must actually
    /// change, never an identity mapping.
    #[test]
    fn uuid_minting_produces_a_well_formed_value_of_the_same_shape() {
        let sources = [
            "550e8400-e29b-41d4-a716-446655440000",
            "6BA7B810-9DAD-11D1-80B4-00C04FD430C8",
            "550e8400e29b41d4a716446655440000",
        ];
        let mut builder = LedgerBuilder::new(EquivalenceClasses::default());
        for source in sources {
            builder.observe_uuid(source);
        }
        let ledger = builder.freeze(&context(1)).expect("freeze");

        for source in sources {
            let minted = ledger.lookup_uuid(source).expect("minted");
            assert_ne!(minted, source, "`{source}` must actually be minted");
            assert_eq!(minted.len(), source.len(), "`{source}` length must survive");
            assert!(
                crate::classify::is_uuid_shaped(minted),
                "`{minted}` is not UUID-shaped"
            );
        }
    }

    /// Distinct UUIDs never collide, and minting is deterministic given the
    /// same seed — the two properties requirement #4 and the determinism
    /// contract respectively demand of the dataset-wide UUID space, exactly
    /// as they already hold for a per-class domain.
    #[test]
    fn uuid_minting_is_injective_and_deterministic() {
        let sources = [
            "550e8400-e29b-41d4-a716-446655440000",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            "00000000-0000-0000-0000-000000000000",
            "ffffffffffffffffffffffffffffffff",
        ];

        let mint_all = |seed: u64| -> BTreeMap<String, String> {
            let mut builder = LedgerBuilder::new(EquivalenceClasses::default());
            for source in sources {
                builder.observe_uuid(source);
            }
            builder
                .freeze(&context(seed))
                .expect("freeze")
                .uuid_mappings
        };

        let first = mint_all(9);
        let second = mint_all(9);
        assert_eq!(first, second, "the same seed must mint deterministically");

        let minted: BTreeSet<&String> = first.values().collect();
        assert_eq!(
            minted.len(),
            sources.len(),
            "minted UUIDs must be pairwise distinct"
        );
    }

    /// The same UUID observed through two different columns — key or not,
    /// FK-linked or not — must land on one shared entry in the dataset-wide
    /// domain rather than one per occurrence.
    #[test]
    fn repeated_observations_of_the_same_uuid_collapse_to_one_domain_entry() {
        let mut builder = LedgerBuilder::new(EquivalenceClasses::default());
        builder.observe_uuid("550e8400-e29b-41d4-a716-446655440000");
        builder.observe_uuid("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(builder.uuid_domain.len(), 1);
    }

    /// A non-UUID-shaped value in a `MintUuid`-routed column must never
    /// enter the dataset-wide domain — observing it would mint
    /// it verbatim-adjacent to real UUIDs instead of letting it fall back to
    /// redaction.
    #[test]
    fn a_non_uuid_shaped_column_value_is_never_observed_into_the_uuid_domain() {
        let dataset = Dataset {
            tables: vec![table(
                "audit",
                vec![text_pk_column(
                    "entity_uuid",
                    &["550e8400-e29b-41d4-a716-446655440000", "not-a-uuid"],
                )],
            )],
        };
        let classes = classes(&dataset);
        let mut uuid_columns = BTreeSet::new();
        uuid_columns.insert("audit.entity_uuid".to_owned());

        let mut builder = LedgerBuilder::new(classes);
        builder.observe_dataset(&dataset, &uuid_columns, &BTreeSet::new());

        assert_eq!(builder.uuid_domain.len(), 1);
        assert_eq!(
            builder.uuid_domain[0],
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    /// A preserved column's values must not be observed at all, since
    /// [`crate::transform::Strategy::Keep`] never mints them.
    #[test]
    fn a_preserved_key_column_is_never_observed() {
        let dataset = Dataset {
            tables: vec![table("t", vec![int_pk_column("id", &[1, 2, 3])])],
        };
        let classes = classes(&dataset);
        let mut preserved = BTreeSet::new();
        preserved.insert("t.id".to_owned());

        let mut builder = LedgerBuilder::new(classes);
        builder.observe_dataset(&dataset, &BTreeSet::new(), &preserved);

        assert!(builder.domains.is_empty());
    }

    fn frozen_with_documents(
        dataset: &Dataset,
        seed: u64,
        documents: &[(&str, KeyDocument)],
    ) -> (EquivalenceClasses, Ledger) {
        let classes = classes(dataset);
        let mut builder = LedgerBuilder::new(classes.clone());
        builder.observe_dataset(dataset, &BTreeSet::new(), &BTreeSet::new());
        for &(qualified, document) in documents {
            let class = classes
                .class_of(qualified)
                .expect("documented column is a key");
            builder.documents.insert(class, document);
        }
        let ledger = match builder.freeze(&context(seed)) {
            Ok(ledger) => ledger,
            Err(error) => panic!("freeze failed: {error}"),
        };
        (classes, ledger)
    }

    fn mapping_of<'a>(
        classes: &EquivalenceClasses,
        ledger: &'a Ledger,
        qualified: &str,
    ) -> &'a BTreeMap<KeyValue, KeyValue> {
        let class = classes.class_of(qualified).expect("column is a key");
        &ledger.mappings[&class]
    }

    fn digits(n: u64, width: usize) -> Vec<u32> {
        format!("{n:0width$}")
            .chars()
            .filter_map(|c| c.to_digit(10))
            .collect()
    }

    fn integer_of(digits: &[u32]) -> i64 {
        digits.iter().fold(0i64, |acc, &d| acc * 10 + i64::from(d))
    }

    /// The CPF whose nine-digit body is `body`, stored as an integer.
    fn cpf(body: u64) -> i64 {
        let mut body = digits(body, 9);
        body.extend(classify::document_check_digits(&body, false));
        integer_of(&body)
    }

    /// The CNPJ whose twelve-digit body is `body`, stored as an integer.
    fn cnpj(body: u64) -> i64 {
        let mut body = digits(body, 12);
        body.extend(classify::document_check_digits(&body, true));
        integer_of(&body)
    }

    fn minted_int(mapping: &BTreeMap<KeyValue, KeyValue>, source: i64) -> i64 {
        match mapping[&KeyValue::Int(source)] {
            KeyValue::Int(minted) => minted,
            ref other => panic!("{source} minted to {other:?}, not an integer"),
        }
    }

    fn minted_text<'a>(mapping: &'a BTreeMap<KeyValue, KeyValue>, source: &str) -> &'a str {
        match &mapping[&KeyValue::Text(source.to_owned())] {
            KeyValue::Text(minted) => minted,
            other => panic!("`{source}` minted to {other:?}, not text"),
        }
    }

    #[test]
    fn valid_integer_documents_mint_valid_documents_of_the_same_width() {
        let cpfs = [
            cpf(123_456_789),
            cpf(529_982_247),
            cpf(12_345_678),
            cpf(1_234),
        ];
        let cnpjs = [cnpj(112_223_330_001), cnpj(1_000_001), cnpj(1)];
        assert_eq!(digit_width(cpfs[2]), 10, "fixture drops one leading zero");
        assert_eq!(digit_width(cnpjs[1]), 9, "fixture drops five leading zeros");
        assert_eq!(cnpjs[2], 191);
        let dataset = Dataset {
            tables: vec![
                table("clientes", vec![int_pk_column("cpf", &cpfs)]),
                table("empresas", vec![int_pk_column("cnpj", &cnpjs)]),
            ],
        };
        for seed in 0..10u64 {
            let (classes, ledger) = frozen_with_documents(
                &dataset,
                seed,
                &[
                    ("clientes.cpf", KeyDocument::Cpf),
                    ("empresas.cnpj", KeyDocument::Cnpj),
                ],
            );
            for (qualified, document, sources) in [
                ("clientes.cpf", KeyDocument::Cpf, &cpfs[..]),
                ("empresas.cnpj", KeyDocument::Cnpj, &cnpjs[..]),
            ] {
                let mapping = mapping_of(&classes, &ledger, qualified);
                for &source in sources {
                    assert!(document.is_valid(&KeyValue::Int(source)));
                    let minted = minted_int(mapping, source);
                    assert!(
                        document.is_valid(&KeyValue::Int(minted)),
                        "seed {seed}: valid {source} minted to {minted}, not a valid {document:?}"
                    );
                    assert_eq!(
                        digit_width(minted),
                        digit_width(source),
                        "seed {seed}: {source} minted to {minted}, changing its width"
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_integer_documents_mint_malformed_documents_of_the_same_sign_and_width() {
        let bumped = cpf(123_456_780) + 1;
        let sources = [
            bumped,
            -cpf(123_456_789),
            0,
            5,
            123_456_789_012,
            cpf(529_982_247),
        ];
        for &source in &sources[..5] {
            assert!(
                !KeyDocument::Cpf.is_valid(&KeyValue::Int(source)),
                "fixture {source} must be malformed"
            );
        }
        let dataset = Dataset {
            tables: vec![table("clientes", vec![int_pk_column("cpf", &sources)])],
        };
        for seed in 0..10u64 {
            let (classes, ledger) =
                frozen_with_documents(&dataset, seed, &[("clientes.cpf", KeyDocument::Cpf)]);
            let mapping = mapping_of(&classes, &ledger, "clientes.cpf");
            for &source in &sources[..5] {
                let minted = minted_int(mapping, source);
                assert!(
                    !KeyDocument::Cpf.is_valid(&KeyValue::Int(minted)),
                    "seed {seed}: malformed {source} minted to the valid CPF {minted}"
                );
                assert_eq!(
                    minted.is_negative(),
                    source.is_negative(),
                    "seed {seed}: {source} minted to {minted}, changing its sign"
                );
                assert_eq!(
                    digit_width(minted),
                    digit_width(source),
                    "seed {seed}: {source} minted to {minted}, changing its width"
                );
            }
        }
    }

    #[test]
    fn valid_text_documents_mint_valid_documents_with_every_other_character_in_place() {
        let cpfs = ["529.982.247-25", "11144477735", " 123.456.789-09"];
        let cnpjs = ["11.222.333/0001-81"];
        let dataset = Dataset {
            tables: vec![
                table("clientes", vec![text_pk_column("cpf", &cpfs)]),
                table("empresas", vec![text_pk_column("cnpj", &cnpjs)]),
            ],
        };
        for seed in 0..10u64 {
            let (classes, ledger) = frozen_with_documents(
                &dataset,
                seed,
                &[
                    ("clientes.cpf", KeyDocument::Cpf),
                    ("empresas.cnpj", KeyDocument::Cnpj),
                ],
            );
            for (qualified, document, sources) in [
                ("clientes.cpf", KeyDocument::Cpf, &cpfs[..]),
                ("empresas.cnpj", KeyDocument::Cnpj, &cnpjs[..]),
            ] {
                let mapping = mapping_of(&classes, &ledger, qualified);
                for &source in sources {
                    assert!(document.is_valid(&KeyValue::Text(source.to_owned())));
                    let minted = minted_text(mapping, source);
                    assert!(
                        document.is_valid(&KeyValue::Text(minted.to_owned())),
                        "seed {seed}: `{source}` minted to `{minted}`, not a valid {document:?}"
                    );
                    assert_eq!(minted.chars().count(), source.chars().count());
                    for (index, (original, generated)) in
                        source.chars().zip(minted.chars()).enumerate()
                    {
                        if !original.is_ascii_digit() {
                            assert_eq!(
                                generated, original,
                                "seed {seed}: `{source}` minted to `{minted}`, moving index {index}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_valid_text_document_key_mints_onto_a_source_value_of_its_class() {
        let sources: Vec<String> = (0..300u64)
            .map(|i| {
                let digits = format!("{:011}", cpf(100_000_000 + i * 7));
                format!(
                    "{}.{}.{}-{}",
                    &digits[..3],
                    &digits[3..6],
                    &digits[6..9],
                    &digits[9..]
                )
            })
            .collect();
        let borrowed: Vec<&str> = sources.iter().map(String::as_str).collect();
        let dataset = Dataset {
            tables: vec![table("clientes", vec![text_pk_column("cpf", &borrowed)])],
        };
        let source_values: BTreeSet<KeyValue> =
            sources.iter().cloned().map(KeyValue::Text).collect();
        for seed in 0..10u64 {
            let (classes, ledger) =
                frozen_with_documents(&dataset, seed, &[("clientes.cpf", KeyDocument::Cpf)]);
            for minted in mapping_of(&classes, &ledger, "clientes.cpf").values() {
                assert!(
                    !source_values.contains(minted),
                    "seed {seed}: minted {minted:?} onto a source value of its class"
                );
            }
        }
    }

    /// Every valid three-digit CNPJ (one per leading digit, `191` among them)
    /// fills its cell completely, beside two malformed three-digit values: the
    /// valid ones must permute among themselves, the malformed ones stay
    /// malformed, and the class must not exhaust its domain.
    #[test]
    fn a_full_cell_of_valid_documents_mints_injectively() {
        let valid: Vec<i64> = (1..=9u64).map(cnpj).collect();
        assert!(valid.contains(&191));
        assert!(valid.iter().all(|&n| digit_width(n) == 3));
        let malformed = [192i64, 100];
        for source in malformed {
            assert!(!KeyDocument::Cnpj.is_valid(&KeyValue::Int(source)));
        }
        let sources: Vec<i64> = valid.iter().copied().chain(malformed).collect();
        let dataset = Dataset {
            tables: vec![table("empresas", vec![int_pk_column("cnpj", &sources)])],
        };
        for seed in 0..10u64 {
            let (classes, ledger) =
                frozen_with_documents(&dataset, seed, &[("empresas.cnpj", KeyDocument::Cnpj)]);
            let mapping = mapping_of(&classes, &ledger, "empresas.cnpj");
            let minted_valid: BTreeSet<i64> = valid
                .iter()
                .map(|&source| minted_int(mapping, source))
                .collect();
            let expected: BTreeSet<i64> = valid.iter().copied().collect();
            assert_eq!(
                minted_valid, expected,
                "seed {seed}: valid CNPJs must permute"
            );
            let mut minted_all: BTreeSet<i64> = minted_valid.clone();
            for source in malformed {
                let minted = minted_int(mapping, source);
                assert!(!KeyDocument::Cnpj.is_valid(&KeyValue::Int(minted)));
                assert_eq!(digit_width(minted), 3);
                minted_all.insert(minted);
            }
            assert_eq!(
                minted_all.len(),
                sources.len(),
                "seed {seed}: not injective"
            );
        }
    }

    #[test]
    fn thousands_of_valid_integer_cpfs_mint_injectively() {
        let sources: Vec<i64> = (0..5_000u64).map(|i| cpf(100_000_000 + i * 131)).collect();
        let dataset = Dataset {
            tables: vec![table("clientes", vec![int_pk_column("cpf", &sources)])],
        };
        let (classes, ledger) =
            frozen_with_documents(&dataset, 11, &[("clientes.cpf", KeyDocument::Cpf)]);
        let mapping = mapping_of(&classes, &ledger, "clientes.cpf");
        let minted: BTreeSet<&KeyValue> = mapping.values().collect();
        assert_eq!(minted.len(), sources.len());
        assert!(minted.iter().all(|key| KeyDocument::Cpf.is_valid(key)));
    }

    #[test]
    fn minting_documents_at_the_same_seed_is_deterministic() {
        let dataset = Dataset {
            tables: vec![
                table(
                    "clientes",
                    vec![int_pk_column(
                        "cpf",
                        &[cpf(123_456_789), cpf(1_234) + 1, -5],
                    )],
                ),
                table(
                    "empresas",
                    vec![text_pk_column("cnpj", &["11.222.333/0001-81"])],
                ),
            ],
        };
        let documents = [
            ("clientes.cpf", KeyDocument::Cpf),
            ("empresas.cnpj", KeyDocument::Cnpj),
        ];
        let (_, first) = frozen_with_documents(&dataset, 21, &documents);
        let (_, second) = frozen_with_documents(&dataset, 21, &documents);
        assert_eq!(first.mappings, second.mappings);
    }

    /// The same values mint identically in a class with no document type
    /// whether or not another class of the ledger has one.
    #[test]
    fn a_class_without_a_document_type_mints_exactly_as_without_any_document() {
        let values = [cpf(123_456_789), cpf(529_982_247) + 1, 42, -7, 0];
        let dataset = Dataset {
            tables: vec![
                table("contas", vec![int_pk_column("id", &values)]),
                table("clientes", vec![int_pk_column("cpf", &values)]),
            ],
        };
        for seed in 0..10u64 {
            let (classes, plain) = frozen_with_documents(&dataset, seed, &[]);
            let (_, documented) =
                frozen_with_documents(&dataset, seed, &[("clientes.cpf", KeyDocument::Cpf)]);
            assert_eq!(
                mapping_of(&classes, &plain, "contas.id"),
                mapping_of(&classes, &documented, "contas.id"),
                "seed {seed}: a class without a document type changed its minted output"
            );
        }
    }

    fn length_three(_: usize) -> usize {
        3
    }

    fn every_length_three_handle() -> Vec<String> {
        let mut handles = Vec::new();
        for first in 'a'..='z' {
            for middle in "abcdefghijklmnopqrstuvwxyz0123456789".chars() {
                for last in '0'..='9' {
                    handles.push(format!("{first}{middle}{last}"));
                }
            }
        }
        handles
    }

    fn is_handle_shaped(handle: &str) -> bool {
        let chars: Vec<char> = handle.chars().collect();
        chars.first().is_some_and(char::is_ascii_lowercase)
            && chars.last().is_some_and(char::is_ascii_digit)
            && chars
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    }

    #[test]
    fn every_text_value_of_a_handle_column_is_observed_whatever_its_shape() {
        let mut column = text_pk_column("login", &["bob", "N/A", " ana ", "", "bob"]);
        column.spec.pk = false;
        column.values.push(Value::Null);
        let dataset = Dataset {
            tables: vec![table("users", vec![column])],
        };
        let handle_columns: BTreeSet<String> = ["users.login".to_owned()].into();

        let mut builder = LedgerBuilder::new(EquivalenceClasses::default());
        builder.observe_handle_columns(&dataset, &handle_columns);

        assert_eq!(builder.handle_domain, ["bob", "N/A", " ana ", ""]);
    }

    #[test]
    fn a_minted_handle_is_lowercase_alphanumeric_from_a_letter_to_a_digit() {
        for (source_len, expected_len) in [(1, 8), (3, 8), (8, 8), (20, 20), (32, 32), (40, 32)] {
            assert_eq!(handle_len(source_len), expected_len);
        }
        let mut rng = context(1).rng_for("handle-shape");
        for len in [8usize, 12, 20, 32] {
            for _ in 0..200 {
                let handle = mint_handle_text(len, &mut rng);
                assert_eq!(handle.chars().count(), len);
                assert!(is_handle_shaped(&handle), "`{handle}` is not handle-shaped");
            }
        }
    }

    #[test]
    fn freezing_the_same_handle_domain_at_the_same_seed_is_deterministic() {
        let mint_all = |seed: u64| -> BTreeMap<String, String> {
            let mut builder = LedgerBuilder::new(EquivalenceClasses::default());
            builder.observe_handle("bob");
            builder.observe_handle("ana.costa1");
            match builder.freeze(&context(seed)) {
                Ok(ledger) => ledger.handle_mappings,
                Err(error) => panic!("freeze failed: {error}"),
            }
        };
        let first = mint_all(13);
        assert_eq!(first, mint_all(13));
        assert_eq!(first.values().collect::<BTreeSet<_>>().len(), 2);
    }

    /// 2000 sources into the 9360 length-three handles: about 200 draws
    /// collide, so a mint without the retry would repeat a handle.
    #[test]
    fn handle_minting_is_injective_under_real_collision_pressure() {
        let domain: Vec<String> = (0..2000).map(|i| format!("source-{i}")).collect();
        let mappings = match mint_handle_mappings(&context(1), &domain, length_three) {
            Ok(mappings) => mappings,
            Err(error) => panic!("minting failed: {error}"),
        };
        let minted: BTreeSet<&String> = mappings.values().collect();
        assert_eq!(minted.len(), domain.len());
    }

    /// Sources that are uppercase spellings of 4000 of the 9360
    /// length-three handles: each draw lands on one of them about 43% of
    /// the time, so a mint that did not avoid the sources would emit one.
    #[test]
    fn a_minted_handle_never_equals_a_source_case_insensitively() {
        let domain: Vec<String> = every_length_three_handle()
            .into_iter()
            .step_by(2)
            .take(4000)
            .map(|handle| handle.to_uppercase())
            .collect();
        let sources: BTreeSet<String> = domain.iter().map(|s| s.to_lowercase()).collect();
        let mappings = match mint_handle_mappings(&context(2), &domain, length_three) {
            Ok(mappings) => mappings,
            Err(error) => panic!("minting failed: {error}"),
        };
        assert_eq!(
            mappings.values().collect::<BTreeSet<_>>().len(),
            domain.len()
        );
        for minted in mappings.values() {
            assert!(!sources.contains(minted), "`{minted}` equals a source");
        }
    }

    #[test]
    fn handle_minting_fails_loudly_once_the_domain_is_exhausted() {
        let used: BTreeSet<String> = every_length_three_handle().into_iter().collect();
        assert_eq!(used.len(), 26 * 36 * 10);
        let error = mint_handle_at_len(&context(1), "one-more-source", 3, &used)
            .expect_err("every length-three handle is already used");
        assert!(matches!(error, Error::HandleDomainExhausted));
    }
}
