use std::collections::BTreeMap;

use anonymizer::dataset::{ColumnData, ColumnSpec, ColumnType, ForeignKey, KeyValue};
use anonymizer::{Dataset, Policy, Table, Value};

fn pk_column(name: &str, values: &[i64]) -> ColumnData {
    ColumnData {
        spec: ColumnSpec {
            name: name.to_owned(),
            ty: ColumnType::Integer,
            nullable: false,
            pk: true,
            fk: None,
        },
        values: values.iter().copied().map(Value::Int).collect(),
    }
}

fn fk_column(name: &str, target_table: &str, target_column: &str, values: &[i64]) -> ColumnData {
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
        values: values.iter().copied().map(Value::Int).collect(),
    }
}

/// Three tables chained two FK hops deep: `order_items.order_id` ->
/// `orders.id`, and `orders.customer_id` -> `customers.id`. Each hop is its
/// own equivalence class (nothing unions `orders.id` with `customers.id`),
/// so this exercises the ledger across two independent classes at once
/// rather than collapsing to a single-class scenario.
fn fixture() -> Dataset {
    Dataset {
        tables: vec![
            Table {
                name: "customers".into(),
                columns: vec![pk_column("id", &[1, 2, 3, 4])],
            },
            Table {
                name: "orders".into(),
                columns: vec![
                    pk_column("id", &[100, 101, 102, 103, 104, 105]),
                    fk_column("customer_id", "customers", "id", &[1, 1, 1, 2, 3, 3]),
                ],
            },
            Table {
                name: "order_items".into(),
                columns: vec![
                    pk_column("id", &[1000, 1001, 1002, 1003, 1004, 1005, 1006]),
                    fk_column(
                        "order_id",
                        "orders",
                        "id",
                        &[100, 100, 101, 102, 103, 105, 105],
                    ),
                ],
            },
        ],
    }
}

fn int_values(table: &Table, column: &str) -> Vec<i64> {
    table
        .column(column)
        .expect("column present")
        .values
        .iter()
        .map(|value| match value {
            Value::Int(n) => *n,
            other => panic!("expected an integer, got {other:?}"),
        })
        .collect()
}

/// Group sizes when grouping `values` by their own value, as a sorted
/// multiset — order-independent, so it only ever asserts the partition
/// *shape*.
fn group_sizes(values: &[i64]) -> Vec<usize> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for &v in values {
        *counts.entry(v).or_insert(0) += 1;
    }
    let mut sizes: Vec<usize> = counts.into_values().collect();
    sizes.sort_unstable();
    sizes
}

/// Builds the before -> after mapping implied by zipping two equal-length
/// FK columns row by row, then asserts it is both a well-defined function
/// (the same source value always maps to the same target) and injective
/// (two distinct source values never collapse onto the same target) — the
/// two halves of requirement #4 that the ledger exists to guarantee.
fn assert_consistent_and_injective(before: &[i64], after: &[i64]) {
    assert_eq!(before.len(), after.len());

    let mut forward: BTreeMap<i64, i64> = BTreeMap::new();
    for (&b, &a) in before.iter().zip(after) {
        match forward.get(&b) {
            Some(&existing) => assert_eq!(
                existing, a,
                "source value {b} mapped to two different targets: {existing} and {a}"
            ),
            None => {
                forward.insert(b, a);
            }
        }
    }

    let distinct_sources = forward.len();
    let distinct_targets: std::collections::BTreeSet<i64> = forward.values().copied().collect();
    assert_eq!(
        distinct_targets.len(),
        distinct_sources,
        "minting must be injective: {distinct_sources} distinct source keys produced only \
         {} distinct targets",
        distinct_targets.len()
    );
}

/// Functional test #2: the FK partition is isomorphic across a two-hop
/// chain. The multiset of group sizes survives (requirement #2), and the
/// source -> target key mapping is injective (requirement #4) — the
/// failure the ledger exists to prevent, not merely a byproduct of
/// preserving sizes.
#[test]
fn fk_partition_is_isomorphic_across_a_two_hop_chain() {
    let dataset = fixture();
    let policy = Policy {
        seed: Some(2024),
        ..Default::default()
    };
    let outcome = anonymizer::anonymize(dataset.clone(), policy).expect("anonymize");

    let before_orders = dataset.table("orders").expect("orders");
    let after_orders = outcome.dataset.table("orders").expect("orders");
    let before_customer_id = int_values(before_orders, "customer_id");
    let after_customer_id = int_values(after_orders, "customer_id");

    assert_eq!(
        group_sizes(&before_customer_id),
        group_sizes(&after_customer_id),
        "orders grouped by customer_id must preserve group sizes"
    );
    assert_consistent_and_injective(&before_customer_id, &after_customer_id);

    let before_items = dataset.table("order_items").expect("order_items");
    let after_items = outcome.dataset.table("order_items").expect("order_items");
    let before_order_id = int_values(before_items, "order_id");
    let after_order_id = int_values(after_items, "order_id");

    assert_eq!(
        group_sizes(&before_order_id),
        group_sizes(&after_order_id),
        "order_items grouped by order_id must preserve group sizes"
    );
    assert_consistent_and_injective(&before_order_id, &after_order_id);

    // The two hops are independent classes: customers.id/orders.customer_id
    // mint through one mapping, orders.id/order_items.order_id through
    // another. Confirm orders.id itself round-trips consistently with what
    // order_items.order_id was mapped to, i.e. the two classes agree on
    // the same table's PK.
    let before_orders_id = int_values(before_orders, "id");
    let after_orders_id = int_values(after_orders, "id");
    let mut orders_pk_map: BTreeMap<i64, i64> = BTreeMap::new();
    for (&b, &a) in before_orders_id.iter().zip(&after_orders_id) {
        orders_pk_map.insert(b, a);
    }
    for (&before_fk, &after_fk) in before_order_id.iter().zip(&after_order_id) {
        assert_eq!(
            orders_pk_map[&before_fk], after_fk,
            "order_items.order_id must mint to exactly what orders.id minted to"
        );
    }
}

/// A dangling FK value (no matching parent row) must still be minted, and
/// must still read as dangling in the output: the orphan survives, it just
/// wears a different, consistently-minted value.
#[test]
fn an_orphan_fk_value_survives_as_an_orphan() {
    let dataset = Dataset {
        tables: vec![
            Table {
                name: "customers".into(),
                columns: vec![pk_column("id", &[1, 2, 3])],
            },
            Table {
                name: "orders".into(),
                columns: vec![
                    pk_column("id", &[10, 11, 12]),
                    fk_column("customer_id", "customers", "id", &[1, 2, 999]),
                ],
            },
        ],
    };

    let outcome = anonymizer::anonymize(dataset, Policy::default()).expect("anonymize");

    let customers = outcome.dataset.table("customers").expect("customers");
    let orders = outcome.dataset.table("orders").expect("orders");
    let customer_ids: std::collections::BTreeSet<KeyValue> = customers
        .column("id")
        .expect("id")
        .values
        .iter()
        .filter_map(KeyValue::from_value)
        .collect();
    let order_customer_ids = int_values(orders, "customer_id");

    let orphan_count = order_customer_ids
        .iter()
        .filter(|&&id| !customer_ids.contains(&KeyValue::Int(id)))
        .count();
    assert_eq!(
        orphan_count, 1,
        "exactly one orphan must survive in the output"
    );
}
