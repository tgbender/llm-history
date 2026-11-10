use crate::{FieldError, JsonRecord, Presence};
use serde_json::value::RawValue;

/// Open native Claude Code record. Unfamiliar record types and fields remain intact.
#[derive(Debug, Clone)]
pub struct Record {
    native: JsonRecord,
}

impl Record {
    pub fn parse(source: &str) -> Result<Self, serde_json::Error> {
        JsonRecord::parse(source).map(|native| Self { native })
    }

    pub fn native(&self) -> &JsonRecord {
        &self.native
    }

    pub fn record_type(&self) -> Result<Presence<String>, FieldError> {
        self.native.field("type")
    }

    pub fn entry_id(&self) -> Result<Presence<String>, FieldError> {
        self.native.field("uuid")
    }

    pub fn parent_id(&self) -> Result<Presence<String>, FieldError> {
        self.native.field("parentUuid")
    }

    pub fn session_id(&self) -> Result<Presence<String>, FieldError> {
        self.native.field("sessionId")
    }

    pub fn timestamp(&self) -> Result<Presence<String>, FieldError> {
        self.native.field("timestamp")
    }

    pub fn message(&self) -> Result<Presence<JsonRecord>, FieldError> {
        self.native.object("message")
    }

    /// Fields outside this reader's small known envelope, retained as raw JSON.
    pub fn extras(&self) -> impl Iterator<Item = (&str, &RawValue)> {
        self.native.extras(&[
            "type",
            "uuid",
            "parentUuid",
            "sessionId",
            "timestamp",
            "message",
        ])
    }
}
