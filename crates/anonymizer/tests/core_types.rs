use anonymizer::dataset::{KeyValue, Value};

#[test]
fn only_integers_and_text_are_admissible_keys() {
    assert_eq!(KeyValue::from_value(&Value::Int(7)), Some(KeyValue::Int(7)));
    assert_eq!(
        KeyValue::from_value(&Value::Text("abc".into())),
        Some(KeyValue::Text("abc".into()))
    );
    assert_eq!(KeyValue::from_value(&Value::Float(1.0)), None);
    assert_eq!(KeyValue::from_value(&Value::Bool(true)), None);
    assert_eq!(KeyValue::from_value(&Value::Null), None);
}

#[test]
fn values_round_trip_through_json() {
    let values = vec![
        Value::Null,
        Value::Bool(true),
        Value::Int(-3),
        Value::Float(1.5),
        Value::Text("olá".into()),
    ];
    let json = serde_json::to_string(&values).expect("serialize");
    assert_eq!(json, r#"[null,true,-3,1.5,"olá"]"#);
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(&json).expect("deserialize"),
        values
    );
}
