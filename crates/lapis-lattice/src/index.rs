use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use regex::Regex;
use rusqlite::{Connection, params};

use crate::graph::parse_wikilinks;
use crate::{IndexReport, Result};

const SKIP: &[&str] = &[".lapis", ".git", ".obsidian", "node_modules", ".venv", "target"];

pub fn reindex(conn: &Connection, vault: &Path) -> Result<IndexReport> {
    conn.execute_batch("DELETE FROM edges; DELETE FROM chunks; DELETE FROM documents;")?;
    // chunks_fts is an external-content table; the cheapest way to clear it in
    // bulk is to drop and recreate. Single-path updates use the 'delete'
    // command instead (see `forget_path`), which is O(chunks in that file).
    conn.execute_batch("DROP TABLE IF EXISTS chunks_fts;")?;
    conn.execute_batch(
        r#"CREATE VIRTUAL TABLE chunks_fts USING fts5(
            text, path UNINDEXED, heading UNINDEXED,
            content='chunks', content_rowid='chunk_id', tokenize='porter'
        );"#,
    )?;

    let files = walk(vault)?;
    let mut chunks_n = 0u64;
    for rel in &files {
        chunks_n += index_one(conn, vault, rel)?;
    }
    resolve_edges(conn, &files)?;
    crate::sqlite::meta_set(conn, "graph_built", "1")?;
    let documents: u64 =
        conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get::<_, i64>(0)).map(|n| n as u64)?;
    let edges: u64 =
        conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get::<_, i64>(0)).map(|n| n as u64)?;
    Ok(IndexReport { documents, chunks: chunks_n, edges, embedded: 0, embed_error: None })
}

/// Reindex exactly one vault-relative path. A file that no longer exists is
/// dropped from the index. Edge resolution reruns across the vault file list so
/// a new note can satisfy someone else's dangling link, but nothing else is
/// re-read or re-chunked.
pub fn reindex_path(conn: &Connection, vault: &Path, rel: &str) -> Result<IndexReport> {
    let rel = rel.trim_start_matches("./").replace('\\', "/");
    forget_path(conn, &rel)?;
    let abs = vault.join(&rel);
    let mut documents = 0u64;
    let mut chunks_n = 0u64;
    if abs.is_file() {
        chunks_n = index_one(conn, vault, &rel)?;
        documents = 1;
    }
    let files = walk(vault)?;
    resolve_edges(conn, &files)?;
    crate::sqlite::meta_set(conn, "graph_built", "1")?;
    let edges: u64 = conn
        .query_row("SELECT COUNT(*) FROM edges WHERE src = ?1", params![rel], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)?;
    Ok(IndexReport { documents, chunks: chunks_n, edges, embedded: 0, embed_error: None })
}

/// Remove one path from documents, chunks, chunks_fts and edges. The FTS rows
/// must be retired with the external-content 'delete' command *before* the
/// chunk rows go, or the index keeps stale postings.
fn forget_path(conn: &Connection, rel: &str) -> Result<()> {
    let mut stmt = conn.prepare("SELECT chunk_id, text, path, heading FROM chunks WHERE path = ?1")?;
    let rows: Vec<(i64, String, String, Option<String>)> = stmt
        .query_map(params![rel], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .filter_map(|r| r.ok())
        .collect();
    for (id, text, path, heading) in rows {
        conn.execute(
            "INSERT INTO chunks_fts(chunks_fts, rowid, text, path, heading) VALUES('delete',?1,?2,?3,?4)",
            params![id, text, path, heading],
        )?;
    }
    conn.execute("DELETE FROM chunks WHERE path = ?1", params![rel])?;
    conn.execute("DELETE FROM edges WHERE src = ?1", params![rel])?;
    conn.execute("DELETE FROM documents WHERE path = ?1", params![rel])?;
    Ok(())
}

/// Index one file: document row, chunk rows, matching FTS rows, outbound edges.
/// Returns the number of chunks written.
fn index_one(conn: &Connection, vault: &Path, rel: &str) -> Result<u64> {
    let abs = vault.join(rel);
    let meta = fs::metadata(&abs)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let text = read_lossy(&abs);
    let (fm, body) = split_frontmatter(&text);
    let title = fm.get("name").or_else(|| fm.get("title")).cloned().or_else(|| h1(&body));
    let domain = rel.split('/').next().filter(|s| *s != rel).map(str::to_string);
    let doc_type = fm.get("type").cloned().or_else(|| fm.get("doc_type").cloned());
    let status = fm.get("status").cloned();
    let priority = fm.get("priority").cloned();
    let tags_json = tags_json(fm.get("tags"));
    conn.execute(
        "INSERT OR REPLACE INTO documents(path,title,domain,doc_type,status,priority,tags_json,mtime,hash,kind)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'markdown')",
        params![rel, title, domain, doc_type, status, priority, tags_json, mtime, hash(&text)],
    )?;
    let mut n = 0u64;
    for (i, (heading, chunk)) in chunk_body(&body).into_iter().enumerate() {
        conn.execute(
            "INSERT INTO chunks(path, chunk_index, heading, text) VALUES(?1,?2,?3,?4)",
            params![rel, i as i64, heading, chunk],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO chunks_fts(rowid, text, path, heading) VALUES(?1,?2,?3,?4)",
            params![id, chunk, rel, heading],
        )?;
        n += 1;
    }
    for link in parse_wikilinks(&body) {
        conn.execute(
            "INSERT INTO edges(src, dst_raw, dst_path, alias, anchor, resolved) VALUES(?1,?2,NULL,?3,?4,0)",
            params![rel, link.target, link.alias, link.anchor],
        )?;
    }
    Ok(n)
}

fn walk(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    walk_inner(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_inner(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if SKIP.iter().any(|s| *s == name.as_ref()) {
            continue;
        }
        let path = ent.path();
        if path.is_dir() {
            if SKIP.iter().any(|s| path.ends_with(s)) {
                continue;
            }
            walk_inner(root, &path, out)?;
        } else if name.ends_with(".md") || name.ends_with(".markdown") {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn read_lossy(p: &Path) -> String {
    let mut f = match fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

fn split_frontmatter(text: &str) -> (std::collections::BTreeMap<String, String>, String) {
    let mut map = std::collections::BTreeMap::new();
    let t = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"));
    let Some(rest) = t else { return (map, text.to_string()) };
    let Some(end) = rest.find("\n---") else { return (map, text.to_string()) };
    let yaml = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    for line in yaml.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !k.trim().is_empty() {
                map.insert(k.trim().to_string(), v);
            }
        }
    }
    (map, body)
}

fn tags_json(raw: Option<&String>) -> String {
    let Some(raw) = raw else { return "[]".into() };
    let s = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s)
        && v.is_array()
    {
        return serde_json::to_string(&v).unwrap_or_else(|_| "[]".into());
    }
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    let tags: Vec<String> = inner
        .split(',')
        .map(|t| t.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into())
}

fn h1(body: &str) -> Option<String> {
    body.lines().find_map(|l| l.strip_prefix("# ").map(|s| s.trim().to_string()))
}

fn hash(text: &str) -> String {
    // cheap stable fingerprint; not a crypto hash
    format!(
        "{:016x}",
        (text.len() as u64).wrapping_mul(0x9E37)
            ^ text.bytes().fold(0u64, |a, b| a.rotate_left(5) ^ u64::from(b))
    )
}

fn chunk_body(body: &str) -> Vec<(Option<String>, String)> {
    let mut chunks = Vec::new();
    let mut heading: Option<String> = None;
    let mut buf = String::new();
    for line in body.lines() {
        if let Some(h) = line.strip_prefix("# ") {
            if !buf.trim().is_empty() {
                chunks.push((heading.take(), std::mem::take(&mut buf)));
            }
            heading = Some(h.trim().to_string());
            buf.push_str(line);
            buf.push('\n');
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if !buf.trim().is_empty() {
        chunks.push((heading, buf));
    }
    if chunks.is_empty() {
        chunks.push((None, body.to_string()));
    }
    chunks
}

fn resolve_edges(conn: &Connection, files: &[String]) -> Result<()> {
    let re = Regex::new(r"(?i)^(?:.*/)?([^/]+?)(?:\.md)?$").expect("stem");
    let mut upd = conn.prepare("UPDATE edges SET dst_path = ?1, resolved = 1 WHERE rowid = ?2")?;
    let mut sel = conn.prepare("SELECT rowid, dst_raw FROM edges WHERE resolved = 0")?;
    let rows: Vec<(i64, String)> =
        sel.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
    for (id, raw) in rows {
        let want = raw.trim().trim_end_matches(".md");
        let exact = format!("{want}.md");
        let hit =
            files.iter().find(|f| f.as_str() == exact || f.trim_end_matches(".md") == want).cloned().or_else(
                || {
                    let stem = re.captures(want).and_then(|c| c.get(1)).map(|m| m.as_str().to_string());
                    let stem = stem.unwrap_or_else(|| want.to_string());
                    let matches: Vec<_> = files
                        .iter()
                        .filter(|f| {
                            PathBuf::from(*f)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|s| s.eq_ignore_ascii_case(&stem))
                        })
                        .cloned()
                        .collect();
                    if matches.len() == 1 { matches.into_iter().next() } else { None }
                },
            );
        if let Some(path) = hit {
            upd.execute(params![path, id])?;
        }
    }
    Ok(())
}
