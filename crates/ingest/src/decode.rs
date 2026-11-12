//! Shared record sequencing and the established fact policy.
use crate::{ParserState, ProviderState};
use llm_history_types::{Fact, Harness, Usage};
use serde_json::Value;

pub(crate) fn string(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}
pub(crate) fn count(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}
pub fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if item.is_string() {
                    return item.as_str().map(str::to_owned);
                }
                match item.get("type").and_then(Value::as_str).unwrap_or("text") {
                    "text" | "input_text" | "output_text" => {
                        item.get("text").and_then(Value::as_str).map(str::to_owned)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl ParserState {
    /// A physical record may project multiple facts; `index` disambiguates tool blocks.
    pub fn parse_record(&mut self, harness: Harness, v: &Value, ordinal: u64) -> Vec<Fact> {
        self.provider.ordinal = ordinal.saturating_sub(1);
        self.parse(harness, v)
    }
    pub fn parse(&mut self, harness: Harness, v: &Value) -> Vec<Fact> {
        self.provider.ordinal += 1;
        let mut facts = Vec::new();
        let kind = string(v, "type");
        let mut fact = Fact {
            detail: Default::default(),
            canonical: match harness {
                Harness::Codex => crate::codex::decode::record_metadata(v),
                Harness::Claude => crate::claude_code::decode::record_metadata(v),
                Harness::Cursor => crate::cursor::decode::record_metadata(v),
            },
            index: 0,
            kind: kind.clone(),
            timestamp: string(v, "timestamp"),
            role: String::new(),
            phase: String::new(),
            text: String::new(),
            model: self.model.clone(),
            tool: String::new(),
            call_id: String::new(),
            usage: Usage::default(),
            excluded: false,
            usage_key: String::new(),
            effort: self.effort.clone(),
            usage_eligible: true,
            guardian_prompt: false,
        };
        match harness {
            Harness::Codex => crate::codex::decode::decode(self, v, &kind, &mut fact),
            Harness::Claude => {
                if crate::claude_code::decode::decode(self, v, &kind, &mut fact, &mut facts) {
                    return vec![fact];
                }
            }
            Harness::Cursor => crate::cursor::decode::decode(self, v, &mut fact, &mut facts),
        }
        fact.model = self.model.clone();
        // Guardian answers are all verdicts, some as short as {"outcome":"allow"}.
        fact.canonical.agent_protocol = self.subagent
            && fact.role == "assistant"
            && (self.canonical.kind == "guardian"
                || (fact.text.trim_start().starts_with("{\"risk_level\"")
                    && fact.text.contains("\"user_authorization\"")
                    && fact.text.contains("\"outcome\"")));
        fact.guardian_prompt = self.canonical.kind == "guardian" && fact.role == "user";
        if fact.canonical.turn_id.is_empty() {
            fact.canonical.turn_id = self.turn_id.clone();
        }
        fact.effort = self.effort.clone();
        if format!(
            "{}:{}",
            fact.canonical.native_type, fact.canonical.payload_type
        )
        .contains("compact")
        {
            fact.kind = "compaction".into();
        }
        // Claude marks most injected records with isMeta; the rest, and every Codex
        // injection, are recognized by the tags the harness wraps them in.
        fact.excluded = match harness {
            Harness::Claude => crate::claude_code::decode::excluded(v, &fact),
            Harness::Codex => crate::codex::decode::excluded(&fact),
            Harness::Cursor => fact.excluded,
        };
        fact.detail = self.provider.project(harness, v, &fact, self.token_ordinal);
        facts.insert(0, fact);
        facts
    }
}

impl ProviderState {
    pub(crate) fn project(
        &mut self,
        harness: Harness,
        v: &Value,
        f: &Fact,
        ordinal: u64,
    ) -> llm_history_types::provider::Detail {
        match harness {
            Harness::Codex => crate::codex::decode::detail(self, v, f, ordinal),
            Harness::Claude => crate::claude_code::decode::detail(self, v, f, ordinal),
            Harness::Cursor => crate::cursor::decode::detail(f),
        }
    }
}
