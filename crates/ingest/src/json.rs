use crate::Presence;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::{collections::BTreeMap, error::Error, fmt};

/// A native JSON object and its complete original source, including whitespace.
///
/// Fields remain raw JSON, so unknown numbers, nested variants and nulls survive
/// without conversion through a fixed schema. This type deliberately does not
/// implement `Serialize`: use `source()` to retain the original representation.
/// Duplicate-key lookup follows Serde's last-value behavior; the complete original
/// object (including duplicate keys) remains in `source()`.
#[derive(Debug, Clone)]
pub struct JsonRecord {
    source: Box<str>,
    fields: BTreeMap<String, Box<RawValue>>,
}

impl JsonRecord {
    /// Validate one JSON object. Callers retain their input on a syntax/non-object
    /// error; acquisition policy decides how to archive malformed records.
    pub fn parse(source: &str) -> Result<Self, serde_json::Error> {
        let fields = serde_json::from_str(source)?;
        Ok(Self {
            source: source.into(),
            fields,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Access without interpreting a value or losing its original JSON spelling.
    pub fn raw_field(&self, name: &str) -> Presence<&RawValue> {
        match self.fields.get(name) {
            None => Presence::Missing,
            Some(value) if value.get().trim() == "null" => Presence::Null,
            Some(value) => Presence::Value(value.as_ref()),
        }
    }

    /// Decode a known field without changing the retained native object.
    /// Incompatible field types are errors, rather than generated defaults.
    pub fn field<T: DeserializeOwned>(&self, name: &str) -> Result<Presence<T>, FieldError> {
        match self.raw_field(name) {
            Presence::Missing => Ok(Presence::Missing),
            Presence::Null => Ok(Presence::Null),
            Presence::Value(value) => serde_json::from_str(value.get())
                .map(Presence::Value)
                .map_err(|error| FieldError::new(name, error)),
        }
    }

    pub fn object(&self, name: &str) -> Result<Presence<Self>, FieldError> {
        match self.raw_field(name) {
            Presence::Missing => Ok(Presence::Missing),
            Presence::Null => Ok(Presence::Null),
            Presence::Value(value) => Self::parse(value.get())
                .map(Presence::Value)
                .map_err(|error| FieldError::new(name, error)),
        }
    }

    /// All observed fields, including fields newer than the current reader.
    /// Iterator order is key order; native ordering remains in `source()`.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &RawValue)> {
        self.fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_ref()))
    }

    /// Additional fields beyond the caller's currently understood envelope.
    pub fn extras<'a>(
        &'a self,
        known: &'a [&str],
    ) -> impl Iterator<Item = (&'a str, &'a RawValue)> + 'a {
        self.fields().filter(move |(key, _)| !known.contains(key))
    }
}

#[derive(Debug)]
pub struct FieldError {
    field: String,
    error: serde_json::Error,
}

impl FieldError {
    pub fn new(field: &str, error: serde_json::Error) -> Self {
        Self {
            field: field.into(),
            error,
        }
    }

    pub fn field(&self) -> &str {
        &self.field
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "native field {:?}: {}", self.field, self.error)
    }
}

impl Error for FieldError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}
