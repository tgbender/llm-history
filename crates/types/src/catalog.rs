//! Captured native project metadata, independent of storage.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProject {
    pub native_id: String,
    pub name: String,
    pub kind: String,
    pub roots: Vec<String>,
    pub metadata: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogAssignment {
    pub session: String,
    pub project: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectCatalog {
    pub projects: Vec<CatalogProject>,
    pub assignments: Vec<CatalogAssignment>,
}
