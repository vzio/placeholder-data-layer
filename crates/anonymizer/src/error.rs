use crate::dataset::ColumnType;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("duplicate table `{0}`")]
    DuplicateTable(String),

    #[error("duplicate column `{0}`")]
    DuplicateColumn(String),

    #[error("unknown table `{0}`")]
    UnknownTable(String),

    #[error("unknown column `{0}`")]
    UnknownColumn(String),

    #[error("column `{column}` has {len} values, but table `{table}` has {row_count} rows")]
    RaggedTable {
        table: String,
        column: String,
        len: usize,
        row_count: usize,
    },

    #[error("table `{table}` row {row} has {len} values, but {expected} were declared")]
    RaggedRow {
        table: String,
        row: usize,
        len: usize,
        expected: usize,
    },

    #[error("column `{column}` references `{}.{}`, which does not exist", .target.table, .target.column)]
    DanglingForeignKey {
        column: String,
        target: crate::dataset::ForeignKey,
    },

    #[error("column `{column}` references `{}.{}`, which is not a primary key", .target.table, .target.column)]
    ForeignKeyTargetNotPrimaryKey {
        column: String,
        target: crate::dataset::ForeignKey,
    },

    #[error("column `{column}` is null, but is declared non-nullable")]
    NullNotAllowed { column: String },

    #[error("key column `{column}` is {ty:?}; keys must be integer or text")]
    UnsupportedKeyType { column: String, ty: ColumnType },

    #[error("column `{column}` holds a {found} where {declared:?} was declared")]
    TypeMismatch {
        column: String,
        declared: ColumnType,
        found: &'static str,
    },

    #[error("override targets unknown column `{0}`")]
    UnknownOverride(String),

    #[error("strategy `{strategy:?}` cannot apply to column `{column}`")]
    IncompatibleStrategy {
        column: String,
        strategy: crate::transform::Strategy,
    },

    #[error("the key domain of class {class} is exhausted; cannot mint injectively")]
    KeyDomainExhausted { class: usize },

    #[error("the dataset-wide UUID identity space is exhausted; cannot mint injectively")]
    UuidDomainExhausted,

    #[error("the dataset-wide handle identity space is exhausted; cannot mint injectively")]
    HandleDomainExhausted,

    #[error("preserve targets unknown table or column `{0}`")]
    UnknownPreserve(String),

    #[error(
        "preserve on column `{0}` is refused: it is classified as a direct identifier or a \
         dataset-wide UUID identity; set `force_preserve` to override"
    )]
    PreserveRefused(String),

    #[error("column `{0}` is both preserved and given a strategy override; choose one")]
    PreserveOverrideConflict(String),

    #[error(
        "column `{column}` resolves to a different mint strategy than other members of its \
         equivalence class, breaking referential integrity"
    )]
    InconsistentClassMint { column: String },

    #[error("dataset has no tables; there is nothing to anonymize or certify compliant")]
    EmptyDataset,

    #[error("column `{0}` has no transformed values after the transform stage ran")]
    MissingTransformedColumn(String),

    #[error("column `{0}` has no resolved strategy to report")]
    MissingColumnStrategy(String),
}
