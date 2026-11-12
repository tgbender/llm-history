//! Frozen before extraction: full facts, native metadata and checkpoint JSON.
use anyhow::Result;
use llm_history_ingest::{PARSER_VERSION, ParserState};
use llm_history_types::Harness;
use serde_json::Value;
use std::path::Path;

fn corpus() -> Value {
    serde_json::from_str(include_str!("../../../fixtures/decoder-v8.json")).unwrap()
}

fn assert_record(parser: &mut ParserState, harness: Harness, case: &Value, record: &Value) {
    let facts = parser.parse_record(
        harness,
        &record["input"],
        record["ordinal"].as_u64().unwrap(),
    );
    assert_eq!(
        serde_json::to_value(facts).unwrap(),
        record["facts"],
        "{} record {} facts",
        case["name"],
        record["ordinal"]
    );
    assert_eq!(
        serde_json::to_value(parser).unwrap(),
        record["checkpoint"],
        "{} record {} checkpoint",
        case["name"],
        record["ordinal"]
    );
}

#[test]
fn extracted_decoders_preserve_frozen_facts_and_checkpoint_shapes() -> Result<()> {
    let corpus = corpus();
    assert_eq!(
        PARSER_VERSION,
        corpus["parser_version"].as_u64().unwrap() as u32
    );
    for case in corpus["cases"].as_array().unwrap() {
        let harness: Harness = serde_json::from_value(case["harness"].clone())?;
        let mut parser = ParserState::new(Path::new(case["path"].as_str().unwrap()), harness);
        assert_eq!(
            serde_json::to_value(&parser)?,
            case["initial_checkpoint"],
            "{} initial identity",
            case["name"]
        );
        for record in case["records"].as_array().unwrap() {
            assert_record(&mut parser, harness, case, record);
        }
    }
    Ok(())
}

#[test]
fn pre_move_checkpoints_resume_with_the_same_facts_and_usage_accounting() -> Result<()> {
    for case in corpus()["cases"].as_array().unwrap() {
        let harness: Harness = serde_json::from_value(case["harness"].clone())?;
        let mut checkpoint = &case["initial_checkpoint"];
        let mut usage_seed = Value::Array(vec![]);
        for record in case["records"].as_array().unwrap() {
            let mut parser: ParserState = serde_json::from_value(checkpoint.clone())?;
            // Storage rehydrates already-counted Claude response IDs from facts.
            // That set deliberately remains outside serialized parser checkpoints.
            parser.seen_usage = serde_json::from_value(usage_seed)?;
            let mut expected = record.clone();
            if let Some(facts) = record.get("resumed_facts") {
                expected["facts"] = facts.clone();
            }
            if let Some(checkpoint) = record.get("resumed_checkpoint") {
                expected["checkpoint"] = checkpoint.clone();
            }
            assert_record(&mut parser, harness, case, &expected);
            checkpoint = &record["checkpoint"];
            usage_seed = record["usage_seed"].clone();
        }
    }
    Ok(())
}

#[test]
fn legacy_checkpoint_defaults_remain_readable() -> Result<()> {
    let corpus = corpus();
    for case in corpus["cases"].as_array().unwrap() {
        let mut checkpoint = case["initial_checkpoint"].clone();
        for field in [
            "provider",
            "canonical",
            "agent_id",
            "turn_id",
            "token_ordinal",
            "effort",
            "has_context",
            "last_prompt",
            "first_user_message",
        ] {
            checkpoint.as_object_mut().unwrap().remove(field);
        }
        let parser: ParserState = serde_json::from_value(checkpoint.clone())?;
        let restored = serde_json::to_value(parser)?;
        for (key, value) in checkpoint.as_object().unwrap() {
            assert_eq!(&restored[key], value, "legacy checkpoint {key}");
        }
    }
    Ok(())
}
