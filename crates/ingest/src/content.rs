//! Identify reversible content boundaries in retained native JSONL.
//!
//! The source JSON is never round-tripped through a value tree. Unknown fields,
//! ordering, number spellings and opaque strings remain in the original bytes.
use llm_history_types::{Harness, ingestion::ContentLeaf};
use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;
use std::collections::HashMap;

// A small value costs less to keep inline than to reference. Bound the number
// of references even in a very large replacement-history checkpoint.
const MIN_BODY_BYTES: usize = 16;
const MAX_PARTS: usize = 1024;
const MAX_FIELDS: usize = 1024;
const MAX_ARRAY: usize = 4096;

/// Borrow the original tokens while rejecting ambiguous object lookups.
pub(crate) struct Object<'a>(HashMap<String, &'a RawValue>);

impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReadObject;
        impl<'de> de::Visitor<'de> for ReadObject {
            type Value = Object<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object with unique keys")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields = HashMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    if fields.len() >= MAX_FIELDS {
                        return Err(de::Error::custom("object exceeds content extraction limit"));
                    }
                    if fields.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                }
                Ok(Object(fields))
            }
        }
        deserializer.deserialize_map(ReadObject)
    }
}

struct Items<'a>(Vec<&'a RawValue>);

impl<'de> Deserialize<'de> for Items<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReadItems;
        impl<'de> de::Visitor<'de> for ReadItems {
            type Value = Items<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a bounded array")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    if items.len() >= MAX_ARRAY {
                        return Err(de::Error::custom("array exceeds content extraction limit"));
                    }
                    items.push(item);
                }
                Ok(Items(items))
            }
        }
        deserializer.deserialize_seq(ReadItems)
    }
}

impl<'a> Object<'a> {
    pub(crate) fn get(&self, key: &str) -> Option<&'a RawValue> {
        self.0.get(key).copied()
    }
    pub(crate) fn kind(&self) -> Option<&'a str> {
        self.get("type")
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
    }
}

/// Extract only known text leaves whose JSON string token can be reproduced
/// exactly. Unrecognized and unsafe-to-split material stays inline.
pub fn leaves(harness: Harness, bytes: &[u8]) -> Vec<ContentLeaf> {
    let Ok(object) = serde_json::from_slice::<Object<'_>>(bytes) else {
        return Vec::new();
    };
    let mut reader = Leaves {
        source: bytes,
        leaves: Vec::new(),
    };
    let result = match harness {
        Harness::Codex => crate::codex::content::extract(&mut reader, &object),
        Harness::Claude => crate::claude_code::content::extract(&mut reader, &object),
        Harness::Cursor => crate::cursor::content::extract(&mut reader, &object),
    };
    if result.is_err() {
        return Vec::new();
    }
    reader.leaves.sort_unstable_by_key(|leaf| leaf.token.start);
    reader.leaves
}

pub(crate) struct Leaves<'a> {
    source: &'a [u8],
    leaves: Vec<ContentLeaf>,
}

impl<'a> Leaves<'a> {
    pub(crate) fn text(&mut self, raw: Option<&RawValue>, path: String) {
        if self.leaves.len() >= MAX_PARTS {
            return;
        }
        let Some(raw) = raw.map(RawValue::get) else {
            return;
        };
        if !raw.starts_with('"') || raw.len() < MIN_BODY_BYTES + 2 {
            return;
        }
        let Ok(body) = serde_json::from_str::<String>(raw) else {
            return;
        };
        if body.len() < MIN_BODY_BYTES {
            return;
        }
        let mut token = Vec::with_capacity(raw.len());
        llm_history_core::json_string::append_token(&mut token, &body);
        if token != raw.as_bytes() {
            return;
        }
        let start = raw.as_ptr() as usize - self.source.as_ptr() as usize;
        self.leaves.push(ContentLeaf {
            token: start..start + raw.len(),
            native_path: path,
            body,
        });
    }

    pub(crate) fn blocks(
        &mut self,
        raw: Option<&'a RawValue>,
        path: &str,
        mut leaf: impl FnMut(&mut Self, Object<'a>, &str) -> serde_json::Result<()>,
    ) -> serde_json::Result<()> {
        let Some(raw) = raw else { return Ok(()) };
        if raw.get().starts_with('"') {
            self.text(Some(raw), path.to_owned());
        } else if raw.get().starts_with('[') {
            for (i, item) in serde_json::from_str::<Items<'_>>(raw.get())?
                .0
                .into_iter()
                .enumerate()
            {
                if self.leaves.len() >= MAX_PARTS {
                    break;
                }
                if item.get().starts_with('{') {
                    leaf(
                        self,
                        serde_json::from_str(item.get())?,
                        &format!("{path}/{i}"),
                    )?;
                }
            }
        }
        Ok(())
    }
}
