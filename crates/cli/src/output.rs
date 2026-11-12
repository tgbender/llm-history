//! Compact CLI projections. Internal archive keys are never serialized here.
use llm_history_app::api::search::{MessageExcerpt, SearchResponse, SimilarHit};
use llm_history_types::query::SessionSummary;
use serde::Serialize;
use serde_json::Value;

fn noninteractive(kind: &str) -> Option<&str> {
    (kind != "interactive").then_some(kind)
}

#[derive(Serialize)]
struct MessageRow<'a> {
    id: i64,
    session_id: i64,
    profile: &'a str,
    project: &'a str,
    role: &'a str,
    ts: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
}

impl<'a> From<&'a MessageExcerpt> for MessageRow<'a> {
    fn from(m: &'a MessageExcerpt) -> Self {
        Self {
            id: m.message_no,
            session_id: m.session_no,
            profile: &m.profile,
            project: &m.project,
            role: &m.role,
            ts: m.timestamp_utc.as_deref().unwrap_or(&m.ts),
            text: &m.text,
            kind: noninteractive(&m.session_kind),
        }
    }
}

#[derive(Serialize)]
struct HitRow<'a> {
    #[serde(flatten)]
    message: MessageRow<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    _scores: Option<Value>,
}

#[derive(Serialize)]
pub(crate) struct SearchOutput<'a> {
    results: Vec<HitRow<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    searched: Option<&'a llm_history_types::query::Searched>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<&'a Value>,
}

impl<'a> SearchOutput<'a> {
    pub(crate) fn new(response: &'a SearchResponse, explain: bool) -> Self {
        Self {
            results: response.results.iter().map(|hit| HitRow {
                message: (&hit.message).into(),
                verdict: hit.verdict.as_deref(),
                _scores: explain.then(|| serde_json::json!({"retrieval":hit.score,
                    "semantic":hit.semantic_score,"lexical":hit.lexical_score,"rerank":hit.rerank_score})),
            }).collect(),
            matched: response.matched.filter(|matched| *matched > response.results.len()),
            searched: response.searched.as_ref(),
            details: explain.then_some(&response.details),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct SimilarRow<'a> {
    #[serde(flatten)]
    message: MessageRow<'a>,
    similarity: f64,
}

impl<'a> From<&'a SimilarHit> for SimilarRow<'a> {
    fn from(hit: &'a SimilarHit) -> Self {
        Self {
            message: (&hit.message).into(),
            similarity: (hit.similarity * 10_000.0).round() / 10_000.0,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ContextRow {
    pub id: i64,
    pub role: String,
    pub ts: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<bool>,
}

/// One row of a session read: a numbered user turn, or the last answer without a number.
#[derive(Serialize)]
pub(crate) struct TurnRow {
    pub id: i64,
    pub role: String,
    pub ts: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<usize>,
    /// Characters removed from the middle of a long message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct SessionRow<'a> {
    id: i64,
    profile: &'a str,
    provider: &'a str,
    project: &'a str,
    title: String,
    updated_at: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
}

impl<'a> From<&'a SessionSummary> for SessionRow<'a> {
    fn from(s: &'a SessionSummary) -> Self {
        Self {
            id: s.session_no,
            profile: &s.profile,
            provider: &s.provider,
            project: &s.project,
            title: llm_history_core::redact(&s.title),
            updated_at: s.updated_at.as_deref(),
            kind: noninteractive(&s.session_kind),
        }
    }
}
