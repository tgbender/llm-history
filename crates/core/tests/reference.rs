use anyhow::Result;
use serde_json::Value;

#[test]
fn lexical_aliases_match_frozen_python() -> Result<()> {
    let golden: Value = serde_json::from_str(include_str!("../../../fixtures/reference.json"))?;
    for case in golden["aliases"].as_array().unwrap() {
        assert_eq!(
            llm_history_core::code_aliases(case["text"].as_str().unwrap()),
            case["expected"].as_str().unwrap(),
            "{}",
            case["text"]
        );
    }
    Ok(())
}
