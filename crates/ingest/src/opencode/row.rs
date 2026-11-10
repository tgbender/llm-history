use crate::{FieldError, JsonRecord, Presence};
use serde_json::value::RawValue;

/// A view of a native row supplied by acquisition, not an upstream export format.
/// Table names are open strings to accommodate schema additions.
#[derive(Debug, Clone)]
pub struct Row {
    table: String,
    columns: JsonRecord,
}

impl Row {
    pub fn parse(table: impl Into<String>, columns: &str) -> Result<Self, serde_json::Error> {
        JsonRecord::parse(columns).map(|columns| Self {
            table: table.into(),
            columns,
        })
    }

    pub fn table(&self) -> &str {
        &self.table
    }

    pub fn columns(&self) -> &JsonRecord {
        &self.columns
    }

    /// Decode JSON stored in native SQL TEXT, preserving the JSON value's spelling.
    /// A SQL NULL column is `Presence::Null`; TEXT containing `null` is a supplied
    /// JSON value. Missing columns remain missing. Blob acquisition stays with
    /// the source reader and is not represented by this JSON-TEXT method.
    pub fn json_text_column(&self, name: &str) -> Result<Presence<Box<RawValue>>, FieldError> {
        match self.columns.field::<String>(name)? {
            Presence::Missing => Ok(Presence::Missing),
            Presence::Null => Ok(Presence::Null),
            Presence::Value(value) => RawValue::from_string(value)
                .map(Presence::Value)
                .map_err(|error| FieldError::new(name, error)),
        }
    }
}
