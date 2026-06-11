//! kubectl Table responses (column definitions + rows).

pub fn make_table(columns: serde_json::Value, rows: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "columnDefinitions": columns,
        "rows": rows,
    })
}

/// Build a kubectl-compatible Table from column definitions and JSON items.
pub fn build_table(items: &[serde_json::Value], cols: &[(&str, &str, &str)]) -> serde_json::Value {
    let mut col_defs = Vec::new();
    col_defs.push(serde_json::json!({
        "name": "Name",
        "type": "string",
        "format": "name",
        "description": "Name",
        "priority": 0
    }));
    for (name, json_path, col_type) in cols {
        col_defs.push(serde_json::json!({
            "name": name,
            "type": col_type,
            "jsonPath": json_path,
            "description": name,
            "priority": 0
        }));
    }
    col_defs.push(serde_json::json!({
        "name": "Age",
        "type": "date",
        "description": "Age",
        "priority": 0
    }));

    let now = std::time::SystemTime::now();
    let rows: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            let name = item["metadata"]["name"].as_str().unwrap_or("");
            let age = format_ts_relative(
                item["metadata"]["creationTimestamp"].as_str().unwrap_or(""),
                now,
            );
            let mut cells = vec![serde_json::json!(name)];
            for (_, json_path, _) in cols {
                cells.push(extract_json_path(item, json_path));
            }
            cells.push(serde_json::json!(age));
            serde_json::json!({"cells": cells, "object": item})
        })
        .collect();

    serde_json::json!({
        "kind": "Table",
        "apiVersion": "meta.k8s.io/v1",
        "metadata": {},
        "columnDefinitions": col_defs,
        "rows": rows,
    })
}

/// RFC3339 timestamp → relative age (e.g. "5m", "2h").
pub fn format_ts_relative(ts: &str, now: std::time::SystemTime) -> String {
    if let Some(ts_secs) = crate::config::parse_rfc3339_secs(ts) {
        let now_secs = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let delta = (now_secs - ts_secs).max(0) as u64;
        if delta < 60 {
            return format!("{delta}s");
        }
        if delta < 3600 {
            return format!("{}m", delta / 60);
        }
        if delta < 86400 {
            return format!("{}h", delta / 3600);
        }
        return format!("{}d", delta / 86400);
    }
    ts.to_string()
}

fn extract_json_path(obj: &serde_json::Value, path: &str) -> serde_json::Value {
    if !path.starts_with('.') {
        return serde_json::Value::Null;
    }
    let parts: Vec<&str> = path[1..].split('.').collect();
    let mut current = obj;
    for part in &parts {
        current = match current {
            serde_json::Value::Object(m) => m.get(*part).unwrap_or(&serde_json::Value::Null),
            _ => return serde_json::Value::Null,
        };
    }
    if let Some(s) = current.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return serde_json::json!(n);
        }
    }
    current.clone()
}
