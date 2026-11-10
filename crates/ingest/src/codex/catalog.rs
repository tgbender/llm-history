use llm_history_types::catalog::{CatalogAssignment, CatalogProject, ProjectCatalog};
use serde_json::{Value, json};
pub fn codex_catalog(value: &Value) -> Option<ProjectCatalog> {
    let projects = value.get("local-projects")?.as_object()?;
    let mut catalog = ProjectCatalog::default();
    for (key, item) in projects {
        let roots = item["rootPaths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let id = item["id"].as_str().unwrap_or(key).trim();
        if id.is_empty() || roots.is_empty() {
            continue;
        }
        let name = item["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| llm_history_core::project_name(&roots[0]));
        catalog.projects.push(CatalogProject {
            native_id: id.into(),
            name,
            kind: "local".into(),
            roots,
            metadata: json!({}),
        });
    }
    let known = catalog
        .projects
        .iter()
        .map(|p| p.native_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if let Some(assignments) = value["thread-project-assignments"].as_object() {
        for (session, item) in assignments {
            if item["projectKind"] == "local"
                && let Some(project) = item["projectId"].as_str()
                && known.contains(project)
            {
                catalog.assignments.push(CatalogAssignment {
                    session: session.clone(),
                    project: project.into(),
                });
            }
        }
    }
    Some(catalog)
}
