use llm_history_ingest::{JsonRecord, Presence, opencode::Row};

#[test]
fn transcript_row_metadata_and_open_json_text_remain_separate() {
    let source = r#"{"id":"msg_a","session_id":"ses_a","type":"assistant","seq":4,"time_created":1250,"data":"{\"content\":[{\"type\":\"future.block\",\"number\":1.2300}],\"native\":{\"vendor\":true}}","new_column":{"opaque":null}}"#;
    let row = Row::parse("session_message", source).unwrap();
    assert_eq!(row.table(), "session_message");
    assert_eq!(
        row.columns().field::<u64>("seq").unwrap(),
        Presence::Value(4)
    );
    let Presence::Value(data) = row.json_text_column("data").unwrap() else {
        panic!("JSON data")
    };
    assert!(data.get().contains("1.2300"));
    let message = JsonRecord::parse(data.get()).unwrap();
    assert!(message.source().contains("future.block"));
    assert!(
        row.columns()
            .extras(&["id", "session_id", "type", "seq", "time_created", "data"])
            .any(|(key, _)| key == "new_column")
    );
    assert_eq!(row.columns().source(), source);
}

#[test]
fn sql_null_json_null_and_absent_column_remain_distinct() {
    let row = Row::parse(
        "future_table",
        r#"{"sql_null":null,"json_null":"null","json_array":"[]"}"#,
    )
    .unwrap();
    assert!(matches!(
        row.json_text_column("sql_null").unwrap(),
        Presence::Null
    ));
    assert!(matches!(
        row.json_text_column("missing").unwrap(),
        Presence::Missing
    ));
    let Presence::Value(value) = row.json_text_column("json_null").unwrap() else {
        panic!("JSON null")
    };
    assert_eq!(value.get(), "null");
    let Presence::Value(value) = row.json_text_column("json_array").unwrap() else {
        panic!("JSON array")
    };
    assert_eq!(value.get(), "[]");
    assert_eq!(row.table(), "future_table");
}

#[test]
fn malformed_or_changed_json_columns_report_errors_without_losing_native_data() {
    let source = r#"{"data":"{","changed":{"future":true}}"#;
    let row = Row::parse("session_part", source).unwrap();
    assert_eq!(row.json_text_column("data").unwrap_err().field(), "data");
    assert_eq!(
        row.json_text_column("changed").unwrap_err().field(),
        "changed"
    );
    assert_eq!(row.columns().source(), source);
}
