use rusqlite::Connection;
use serde::Serialize;

use crate::{Error, Result};

pub const ANALYTICS_QUERIES: &[&str] =
    &["inventory", "priority", "tags", "health", "recent", "hubs", "density", "degree", "dangling"];

#[derive(Debug, Clone, Serialize)]
pub struct Analytics {
    pub query: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub count: usize,
}

pub fn run(conn: &Connection, query: &str) -> Result<Analytics> {
    let q = query.trim().to_ascii_lowercase();
    if !ANALYTICS_QUERIES.contains(&q.as_str()) {
        return Err(Error::Usage(format!(
            "unknown analytics query '{query}'; allowed: {}",
            ANALYTICS_QUERIES.join(", ")
        )));
    }
    let sql = match q.as_str() {
        "inventory" => "SELECT kind, COUNT(*) AS documents FROM documents GROUP BY kind ORDER BY 2 DESC",
        "priority" => {
            "SELECT COALESCE(priority,'(none)') AS priority, COUNT(*) AS documents FROM documents GROUP BY 1 ORDER BY 2 DESC"
        }
        "tags" => {
            "SELECT json_each.value AS tag, COUNT(*) AS documents FROM documents, json_each(documents.tags_json) GROUP BY 1 ORDER BY 2 DESC LIMIT 50"
        }
        "health" => {
            "SELECT 'documents' AS metric, COUNT(*) AS value FROM documents UNION ALL SELECT 'edges', COUNT(*) FROM edges UNION ALL SELECT 'dangling', COUNT(*) FROM edges WHERE resolved = 0"
        }
        "recent" => {
            "SELECT path, COALESCE(title,'') AS title, mtime FROM documents ORDER BY mtime DESC LIMIT 50"
        }
        "hubs" => {
            "SELECT path, (SELECT COUNT(*) FROM edges e WHERE e.src = d.path OR e.dst_path = d.path) AS deg FROM documents d ORDER BY deg DESC LIMIT 50"
        }
        "density" => {
            "SELECT (SELECT COUNT(*) FROM edges)*1.0 / MAX((SELECT COUNT(*) FROM documents), 1) AS links_per_document"
        }
        "degree" => {
            "SELECT path, (SELECT COUNT(*) FROM edges e WHERE e.src = d.path) AS out_d, (SELECT COUNT(*) FROM edges e WHERE e.dst_path = d.path) AS in_d FROM documents d ORDER BY out_d + in_d DESC LIMIT 200"
        }
        "dangling" => "SELECT src, dst_raw AS target FROM edges WHERE resolved = 0 LIMIT 300",
        _ => unreachable!(),
    };
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();
    let columns: Vec<String> =
        (0..col_count).map(|i| stmt.column_name(i).unwrap_or("c").to_string()).collect();
    let mut rows_iter = stmt.query([])?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next()? {
        let mut v = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let cell: rusqlite::types::Value = row.get(i)?;
            v.push(match cell {
                rusqlite::types::Value::Null => String::new(),
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Real(n) => format!("{n}"),
                rusqlite::types::Value::Text(s) => s,
                rusqlite::types::Value::Blob(_) => "<blob>".into(),
            });
        }
        rows.push(v);
    }
    let count = rows.len();
    Ok(Analytics { query: q, columns, rows, count })
}
