use serde::{Deserialize, Serialize};

/// A single cell.
///
/// [`Value::Float`] keeps this type off `Eq` and `Hash`, so it can never be a
/// map key. Key columns are restricted to integers and text for that reason;
/// use [`KeyValue`] wherever a value must be hashed, ordered or mapped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(untagged)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    // Always a bare JSON integer on the wire. No JS `number` can hold the
    // full `i64` range losslessly, so the generated binding says `number |
    // string` rather than implying every caller's `JSON.parse` is safe.
    #[cfg_attr(feature = "ts", ts(type = "number | string"))]
    Int(i64),
    Float(f64),
    Text(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// The subset of [`Value`] admissible as a primary or foreign key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyValue {
    Int(i64),
    Text(String),
}

impl KeyValue {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Int(n) => Some(KeyValue::Int(*n)),
            Value::Text(s) => Some(KeyValue::Text(s.clone())),
            _ => None,
        }
    }
}

impl From<KeyValue> for Value {
    fn from(key: KeyValue) -> Self {
        match key {
            KeyValue::Int(n) => Value::Int(n),
            KeyValue::Text(s) => Value::Text(s),
        }
    }
}
