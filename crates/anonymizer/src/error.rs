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

    #[error("column `{column}` references `{}.{}`, which does not exist", .target.table, .target.column)]
    DanglingForeignKey {
        column: String,
        target: crate::dataset::ForeignKey,
    },

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
}
