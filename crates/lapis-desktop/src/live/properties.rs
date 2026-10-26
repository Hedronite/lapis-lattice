//! Frontmatter as display rows: the HAL keys a reader looks for first, values as text
//! with any `[[wikilinks]]` split out so they can be followed.
use serde_json::Value;

/// Keys shown first, in this order. The collapsed card shows the first [`SUMMARY`] rows.
const PRIORITY: [&str; 9] =
    ["title", "type", "doc_type", "status", "domain", "updated", "created", "tags", "parent"];
pub const SUMMARY: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Link { label: String, target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub pieces: Vec<Piece>,
}

/// Every property as a row, priority keys first, the rest in the order the map holds them.
pub fn rows(properties: &Value) -> Vec<Row> {
    let Some(map) = properties.as_object() else { return vec![] };
    let rank = |key: &str| PRIORITY.iter().position(|p| *p == key).unwrap_or(PRIORITY.len());
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| rank(k));
    keys.into_iter().map(|key| Row { key: key.clone(), pieces: pieces(&map[key]) }).collect()
}

fn pieces(value: &Value) -> Vec<Piece> {
    match value {
        Value::String(s) => split_links(s),
        Value::Array(items) => {
            let mut out = Vec::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Text(" · ".into()));
                }
                out.extend(pieces(item));
            }
            out
        }
        Value::Null => vec![Piece::Text("—".into())],
        Value::Object(_) => vec![Piece::Text(value.to_string())],
        other => vec![Piece::Text(other.to_string())],
    }
}

/// `"see [[a/b|B]]"` → text `see `, link `B` → `a/b`. An unclosed `[[` stays text.
fn split_links(s: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("[[") {
        let Some(len) = rest[start + 2..].find("]]") else { break };
        if start > 0 {
            out.push(Piece::Text(rest[..start].into()));
        }
        let inner = &rest[start + 2..start + 2 + len];
        let (target, alias) = inner.split_once('|').map_or((inner, None), |(t, a)| (t, Some(a)));
        let label = alias.unwrap_or_else(|| target.rsplit('/').next().unwrap_or(target));
        out.push(Piece::Link { label: label.trim().into(), target: target.trim().into() });
        rest = &rest[start + 2 + len + 2..];
    }
    if !rest.is_empty() || out.is_empty() {
        out.push(Piece::Text(rest.into()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn priority_keys_lead_and_links_split_out() {
        let props = json!({
            "cross_refs": ["[[projects/demo]]", "[[notes/plan|Plan]]"],
            "status": "live",
            "parent": "[[notes/README]]",
            "type": "note",
            "hal_version": "1.0",
        });
        let rows = rows(&props);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(&keys[..3], ["type", "status", "parent"]);
        let parent = &rows[2].pieces;
        assert_eq!(parent, &[Piece::Link { label: "README".into(), target: "notes/README".into() }]);
        let refs = &rows.iter().find(|r| r.key == "cross_refs").unwrap().pieces;
        assert_eq!(
            refs,
            &[
                Piece::Link { label: "demo".into(), target: "projects/demo".into() },
                Piece::Text(" · ".into()),
                Piece::Link { label: "Plan".into(), target: "notes/plan".into() },
            ]
        );
    }

    #[test]
    fn plain_values_stay_text() {
        assert_eq!(split_links("see [[x and more"), [Piece::Text("see [[x and more".into())]);
        assert_eq!(pieces(&json!(3)), [Piece::Text("3".into())]);
        assert_eq!(pieces(&json!(null)), [Piece::Text("—".into())]);
        assert_eq!(
            pieces(&json!(["a", true])),
            [Piece::Text("a".into()), Piece::Text(" · ".into()), Piece::Text("true".into())]
        );
        assert!(rows(&json!("not a map")).is_empty());
    }
}
