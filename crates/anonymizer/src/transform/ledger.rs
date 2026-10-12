use std::collections::{BTreeMap, BTreeSet};

use rand_chacha::ChaCha8Rng;

use crate::dataset::{Dataset, KeyValue};
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

/// Collects the observed key domain of every class. Minting is insertion-order
/// dependent, so this runs single-threaded in a deterministic order: classes
/// sorted by name, values in row order.
#[derive(Debug, Default)]
pub struct LedgerBuilder {
    pub classes: EquivalenceClasses,
    pub domains: BTreeMap<ClassId, Vec<KeyValue>>,
}

impl LedgerBuilder {
    pub fn new(_classes: EquivalenceClasses) -> Self {
        todo!("P4: ledger build phase")
    }

    pub fn observe(&mut self, _class: ClassId, _value: &KeyValue) {
        todo!("P4: domain collection")
    }

    /// Mints one injective mapping per class, then freezes. The parallel
    /// transform only ever sees `&Ledger`.
    pub fn freeze(self, _rng: &mut ChaCha8Rng) -> Result<Ledger, Error> {
        todo!("P4: seeded permutation with collision retry")
    }
}

/// Frozen, injective, per-class key mapping. Keys are minted rather than
/// hashed: hashing a six-figure key domain into a fixed width collides, and a
/// collision silently merges two parent rows.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub mappings: BTreeMap<ClassId, BTreeMap<KeyValue, KeyValue>>,
}

impl Ledger {
    pub fn lookup(&self, _class: ClassId, _value: &KeyValue) -> Option<&KeyValue> {
        todo!("P4: mapping lookup")
    }
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
}
