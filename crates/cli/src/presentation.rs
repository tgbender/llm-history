//! Shared text output for structured command results.
use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets::NOTHING};
use serde_json::Value;

pub fn emit(value: &Value, json_output: bool) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string(value)?);
        return Ok(());
    }
    if let Some(rows) = value.get("rows").and_then(Value::as_array) {
        let columns = value["columns"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        table(rows, &columns);
        if value["truncated"] == true {
            println!("Results truncated; use --full or raise the output limits.");
        }
    } else if let Some(rows) = value.as_array() {
        table(rows, &[]);
    } else if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            if let Some(rows) = value.as_array() {
                println!("{key}:");
                table(rows, &[]);
            } else if value.is_object() {
                println!("{key}:");
                emit(value, false)?;
            } else {
                println!("{key}: {}", text(value));
            }
        }
    } else {
        println!("{}", text(value));
    }
    Ok(())
}
fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}
fn table(rows: &[Value], selected: &[&str]) {
    if rows.is_empty() {
        println!("No results.");
        return;
    }
    let first = &rows[0];
    let preferred: &[&str] = if !selected.is_empty() {
        selected
    } else if first.get("period").is_some() {
        &[
            "period",
            "profile",
            "model",
            "effort",
            "input",
            "cached",
            "cache_write",
            "uncached",
            "output",
            "thinking",
            "total",
        ]
    } else if first.get("updated_at").is_some() && first.get("provider").is_some() {
        &[
            "id",
            "updated_at",
            "profile",
            "provider",
            "project",
            "title",
            "kind",
        ]
    } else if first.get("enabled").is_some() && first.get("root").is_some() {
        &["name", "harness", "root", "account", "backend", "enabled"]
    } else if first.get("roots").is_some() && first.get("sessions").is_some() {
        &[
            "id", "profile", "source", "name", "kind", "active", "sessions", "roots",
        ]
    } else {
        &[]
    };
    let mut columns = preferred
        .iter()
        .filter(|name| rows.iter().any(|r| r.get(**name).is_some()))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if preferred.is_empty() {
        for row in rows.iter().filter_map(Value::as_object) {
            for key in row.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    if columns.is_empty() {
        for row in rows {
            println!("{}", text(row));
        }
        return;
    }
    let cells = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|key| one_line(&text(&row[key])))
                .collect()
        })
        .collect::<Vec<Vec<_>>>();
    // Only a terminal is narrow: piped output (agents, files) keeps every cell whole.
    let widths = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        fit(&columns, &cells, width)
    } else {
        vec![usize::MAX; columns.len()]
    };
    let mut table = Table::new();
    table
        .load_style(NOTHING)
        .set_content_arrangement(ContentArrangement::Disabled)
        .set_header(&columns);
    for row in cells {
        table.add_row(row.iter().zip(&widths).map(|(cell, w)| shorten(cell, *w)));
    }
    println!("{table}");
}
/// Each row prints on one line: line breaks inside a cell become spaces.
fn one_line(text: &str) -> String {
    if text.contains(['\n', '\r', '\t']) {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        text.to_owned()
    }
}
/// Identifiers wider than this are treated as text and may be shortened.
const IDENTIFIER_CHARS: usize = 44;
/// Shortened text columns keep at least this many characters.
const MIN_TEXT_CHARS: usize = 16;
/// Column widths that fit the terminal. Short columns without spaces (IDs, times,
/// numbers, names) keep their full width; text columns share what is left, the
/// widest giving way first. Rows wider than the terminal stay on one line.
fn fit(columns: &[String], cells: &[Vec<String>], width: usize) -> Vec<usize> {
    let natural = (0..columns.len())
        .map(|i| {
            cells
                .iter()
                .map(|row| row[i].chars().count())
                .chain([columns[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    // comfy-table pads each cell with a space on both sides.
    let total: usize = natural.iter().map(|w| w + 2).sum();
    if total <= width {
        return natural;
    }
    let text = (0..columns.len())
        .filter(|&i| natural[i] > IDENTIFIER_CHARS || cells.iter().any(|row| row[i].contains(' ')))
        .collect::<Vec<_>>();
    let fixed: usize = (0..columns.len())
        .filter(|i| !text.contains(i))
        .map(|i| natural[i] + 2)
        .sum();
    let mut left = width.saturating_sub(fixed + 2 * text.len());
    let mut widths = natural.clone();
    let mut open = text.clone();
    open.sort_by_key(|&i| natural[i]);
    // Max-min fair share: narrow text columns keep their width, wide ones split the rest.
    while let Some(&i) = open.first() {
        let share = (left / open.len()).max(MIN_TEXT_CHARS);
        if natural[i] <= share {
            left = left.saturating_sub(natural[i]);
            open.remove(0);
            continue;
        }
        for &i in &open {
            widths[i] = share.max(columns[i].chars().count()).min(natural[i]);
        }
        break;
    }
    widths
}
/// Shortens to `width` characters with an ellipsis; paths keep their end.
fn shorten(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width || width == 0 {
        return text.to_owned();
    }
    if text.starts_with('/') || text.starts_with('~') {
        let tail: String = text.chars().skip(count - (width - 1)).collect();
        return format!("…{tail}");
    }
    let head: String = text.chars().take(width - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_keep_their_width_and_text_gives_way() {
        let columns = ["thread_id", "title", "cwd"].map(String::from).to_vec();
        let cells = vec![vec![
            "claude:00000000-0000-4000-8000-000000000001".to_owned(),
            "Fix the importer so that long titles do not wrap ".repeat(4),
            "/example/workspace/llm-history-rs/crates/store/src".to_owned(),
        ]];
        let widths = fit(&columns, &cells, 100);
        assert_eq!(widths[0], 43);
        assert!(widths.iter().map(|w| w + 2).sum::<usize>() <= 100);
        let shortened = shorten(&cells[0][2], widths[2]);
        assert!(shortened.starts_with('…') && shortened.ends_with("store/src"));
        assert!(shorten(&cells[0][1], widths[1]).ends_with('…'));
        assert_eq!(one_line("a\n  b"), "a b");
    }
    #[test]
    fn narrow_tables_are_unchanged() {
        let columns = ["id", "title"].map(String::from).to_vec();
        let cells = vec![vec!["1".to_owned(), "short title".to_owned()]];
        assert_eq!(fit(&columns, &cells, 120), vec![2, 11]);
    }
}
