use llm_history_types::catalog::{CatalogProject, ProjectCatalog};
use serde_json::{Value, json};
pub fn claude_catalog(value: &Value) -> Option<ProjectCatalog> {
    let projects = value.get("projects")?.as_object()?;
    let mut catalog = ProjectCatalog::default();
    let mut repos_by_root = std::collections::BTreeMap::<String, Vec<String>>::new();
    if let Some(repos) = value["githubRepoPaths"].as_object() {
        for (slug, roots) in repos {
            for root in roots
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if projects.contains_key(root) {
                    repos_by_root
                        .entry(root.into())
                        .or_default()
                        .push(slug.clone());
                }
            }
        }
    }
    for root in projects.keys().filter(|r| !r.is_empty()) {
        let repos = repos_by_root.remove(root).unwrap_or_default();
        catalog.projects.push(CatalogProject {
            native_id: format!("path:{root}"),
            name: llm_history_core::project_name(root),
            kind: "path".into(),
            roots: vec![root.clone()],
            metadata: json!({"repository_slugs":repos}),
        });
    }
    Some(catalog)
}
