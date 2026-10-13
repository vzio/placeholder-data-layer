use std::collections::{BTreeMap, BTreeSet};

use rand::RngExt;
use rand_chacha::ChaCha8Rng;

use crate::classify;
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
// TODO: correlating the members of a composite primary key with each other is
// out of scope for the PoC; each column of a composite key is unioned only
// through its own foreign key edges.
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

/// Collects the observed key domain of every class, plus the dataset-wide
/// domain of UUID-shaped values that mint outside the class system entirely.
/// Minting is insertion-order dependent, so this runs single-threaded in a
/// deterministic order: classes sorted by name, values in row order; the
/// UUID domain in qualified-column-name order, values in row order within a
/// column.
#[derive(Debug, Default)]
pub struct LedgerBuilder {
    pub classes: EquivalenceClasses,
    pub domains: BTreeMap<ClassId, Vec<KeyValue>>,
    pub uuid_domain: Vec<String>,
    /// Membership check for [`Self::observe`]'s dedup: never iterated, so its
    /// hashing order cannot leak into the deterministic domain order that
    /// `domains` preserves.
    seen: BTreeMap<ClassId, BTreeSet<KeyValue>>,
    uuid_seen: BTreeSet<String>,
}

impl LedgerBuilder {
    pub fn new(classes: EquivalenceClasses) -> Self {
        Self {
            classes,
            domains: BTreeMap::new(),
            uuid_domain: Vec::new(),
            seen: BTreeMap::new(),
            uuid_seen: BTreeSet::new(),
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

    /// Mints one injective mapping per class and one injective mapping over
    /// the dataset-wide UUID domain, then freezes. The parallel transform
    /// only ever sees `&Ledger`.
    pub fn freeze(self, context: &RunContext) -> Result<Ledger, Error> {
        let mut mappings = BTreeMap::new();
        for class_index in 0..self.classes.count {
            let class = ClassId(class_index);
            let Some(domain) = self.domains.get(&class) else {
                continue;
            };
            let mut used: BTreeSet<KeyValue> = BTreeSet::new();
            let mut mapping = BTreeMap::new();
            for source in domain {
                let target = mint(context, class, source, &used)?;
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

        Ok(Ledger {
            mappings,
            uuid_mappings,
        })
    }
}

fn resolve_column<'a>(dataset: &'a Dataset, qualified: &str) -> Option<&'a ColumnData> {
    let (table_name, column_name) = qualified.split_once('.')?;
    dataset.table(table_name)?.column(column_name)
}

/// Frozen, injective key mapping, in two parts: a per-class mapping for
/// ordinary integer and text keys, and one dataset-wide mapping for
/// UUID-shaped text, which mints outside the class system so that the same
/// UUID anywhere in the dataset agrees on the same output regardless of
/// which class — if any — observed it. Keys are minted rather than hashed:
/// hashing a six-figure key domain into a fixed width collides, and a
/// collision silently merges two parent rows.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub mappings: BTreeMap<ClassId, BTreeMap<KeyValue, KeyValue>>,
    pub uuid_mappings: BTreeMap<String, String>,
}

impl Ledger {
    pub fn lookup(&self, class: ClassId, value: &KeyValue) -> Option<&KeyValue> {
        self.mappings.get(&class)?.get(value)
    }

    pub fn lookup_uuid(&self, text: &str) -> Option<&str> {
        self.uuid_mappings.get(text).map(String::as_str)
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
fn mint(
    context: &RunContext,
    class: ClassId,
    source: &KeyValue,
    used: &BTreeSet<KeyValue>,
) -> Result<KeyValue, Error> {
    let cap = mint_attempt_cap(candidate_domain_size(source));
    let mut rng = context.rng_for(&mint_label(class, source));
    for _ in 0..cap {
        let candidate = mint_candidate(source, &mut rng);
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::KeyDomainExhausted { class: class.0 })
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
}
