//! HAL frontmatter: split and parse.
//!
//! SPEC.md § "HAL as a first-class citizen": `hal` is the parsed YAML mapping
//! (unknown keys preserved, file order kept), `halValid` is false when the
//! YAML fails to parse, and the body is returned either way.
//!
//! `gray_matter` does the `---` split. Its bundled YAML engine converts to an
//! unordered `Pod` and errors on bad YAML, so we plug in a no-op engine and
//! hand the raw matter to `yaml_serde` ourselves.

use gray_matter::engine::Engine;
use gray_matter::{Matter, ParsedEntity, Pod};
use serde_json::{Map, Value};

/// Engine that never parses: we only want `gray_matter`'s delimiter logic.
struct RawEngine;

impl Engine for RawEngine {
    fn parse(_content: &str) -> gray_matter::Result<Pod> {
        Ok(Pod::Null)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// Parsed mapping. Empty object when there is no frontmatter or it is invalid.
    pub hal: Map<String, Value>,
    /// `true` when frontmatter was absent or parsed as a YAML mapping.
    /// `false` when it was present but not valid YAML (or not a mapping).
    pub hal_valid: bool,
    /// Raw text between the delimiters, `None` when there was none.
    pub matter: Option<String>,
    /// Everything after the closing delimiter (or the whole input).
    pub body: String,
    /// Parse error text when `hal_valid` is false.
    pub error: Option<String>,
}

pub fn parse(input: &str) -> Parsed {
    let matter: Matter<RawEngine> = Matter::new();
    // RawEngine never fails and `Value::Null` deserializes from `Pod::Null`,
    // so this cannot error; fall back to "no frontmatter" defensively anyway.
    let entity: ParsedEntity<Value> = match matter.parse(input) {
        Ok(e) => e,
        Err(_) => {
            return Parsed {
                hal: Map::new(),
                hal_valid: true,
                matter: None,
                body: input.to_string(),
                error: None,
            };
        }
    };

    if entity.matter.is_empty() {
        return Parsed { hal: Map::new(), hal_valid: true, matter: None, body: entity.content, error: None };
    }

    match yaml_serde::from_str::<yaml_serde::Value>(&entity.matter) {
        Ok(yaml_serde::Value::Mapping(m)) => Parsed {
            hal: mapping_to_json(m),
            hal_valid: true,
            matter: Some(entity.matter),
            body: entity.content,
            error: None,
        },
        Ok(other) => Parsed {
            hal: Map::new(),
            hal_valid: false,
            matter: Some(entity.matter),
            body: entity.content,
            error: Some(format!("frontmatter is not a mapping: {}", yaml_kind(&other))),
        },
        Err(e) => Parsed {
            hal: Map::new(),
            hal_valid: false,
            matter: Some(entity.matter),
            body: entity.content,
            error: Some(e.to_string()),
        },
    }
}

/// Split the original file without trimming its body. Parser-normalized body
/// lengths are not byte offsets: using them to splice a save can copy the
/// first body character into the header or split a UTF-8 character.
pub fn raw_parts(input: &str) -> (&str, &str) {
    let mut lines = input.split_inclusive('\n');
    let Some(first) = lines.next() else { return ("", input) };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return ("", input);
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return input.split_at(offset);
        }
    }
    ("", input)
}

#[cfg(test)]
mod raw_parts_tests {
    use super::raw_parts;

    #[test]
    fn preserves_body_bytes_and_unicode_boundary() {
        for body in ["anchor\n", "漢字\n\n", "\n  indented\n", "", "no newline"] {
            let header = "---\ntitle: test\ncustom: keep\n---\n";
            let file = format!("{header}{body}");
            assert_eq!(raw_parts(&file), (header, body));
            assert_eq!(format!("{}{}", raw_parts(&file).0, raw_parts(&file).1), file);
        }
    }

    #[test]
    fn crlf_and_absent_or_unclosed_header() {
        assert_eq!(
            raw_parts("---\r\ntitle: x\r\n---\r\nbody\r\n"),
            ("---\r\ntitle: x\r\n---\r\n", "body\r\n")
        );
        for file in ["plain\n", "---\nnot closed", "body\n---\nmore"] {
            assert_eq!(raw_parts(file), ("", file));
        }
    }
}

fn yaml_kind(v: &yaml_serde::Value) -> &'static str {
    use yaml_serde::Value::*;
    match v {
        Null => "null",
        Bool(_) => "bool",
        Number(_) => "number",
        String(_) => "string",
        Sequence(_) => "sequence",
        Mapping(_) => "mapping",
        Tagged(_) => "tagged",
    }
}

/// YAML → JSON. Non-string mapping keys are stringified (JSON needs string
/// keys); tags are dropped to their inner value.
pub fn yaml_to_json(v: yaml_serde::Value) -> Value {
    use yaml_serde::Value as Y;
    match v {
        Y::Null => Value::Null,
        Y::Bool(b) => Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        Y::String(s) => Value::String(s),
        Y::Sequence(seq) => Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(m) => Value::Object(mapping_to_json(m)),
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

fn mapping_to_json(m: yaml_serde::Mapping) -> Map<String, Value> {
    let mut out = Map::with_capacity(m.len());
    for (k, v) in m {
        let key = match k {
            yaml_serde::Value::String(s) => s,
            yaml_serde::Value::Bool(b) => b.to_string(),
            yaml_serde::Value::Number(n) => n.to_string(),
            yaml_serde::Value::Null => "null".to_string(),
            other => serde_json::to_string(&yaml_to_json(other)).unwrap_or_default(),
        };
        out.insert(key, yaml_to_json(v));
    }
    out
}

/// Display title: HAL `name`, then `title`, then `None`.
pub fn title_from_hal(hal: &Map<String, Value>) -> Option<String> {
    for key in ["name", "title"] {
        if let Some(Value::String(s)) = hal.get(key) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// HAL `tags`: array of strings or a comma-separated string (schema allows both).
pub fn tags_from_hal(hal: &Map<String, Value>) -> Vec<String> {
    match hal.get("tags") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.trim().trim_start_matches('#').to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|t| t.trim().trim_start_matches('#').to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_LIKE: &str = "---\nname: Lapis · SPEC\ntype: foundry-doc\nhal_version: \"1.0\"\ncross_refs:\n  - \"[[a]]\"\n  - \"[[b]]\"\ntags: [x, y]\n---\n<!--hal:authoritative:yaml-->\n\n# SPEC.md\n\nIntro.\n\n---\n\n## Section\n\nAfter the rule.\n";

    #[test]
    fn parses_mapping_and_keeps_order() {
        let p = parse(SPEC_LIKE);
        assert!(p.hal_valid);
        assert_eq!(p.error, None);
        let keys: Vec<&String> = p.hal.keys().collect();
        assert_eq!(keys, ["name", "type", "hal_version", "cross_refs", "tags"]);
        assert_eq!(p.hal["name"], "Lapis · SPEC");
        // quoted "1.0" stays a string, not a float
        assert_eq!(p.hal["hal_version"], "1.0");
        assert_eq!(p.hal["cross_refs"], serde_json::json!(["[[a]]", "[[b]]"]));
    }

    #[test]
    fn body_keeps_horizontal_rules_after_frontmatter() {
        let p = parse(SPEC_LIKE);
        assert!(p.body.starts_with("<!--hal:authoritative:yaml-->"));
        assert!(p.body.contains("\n---\n"), "a `---` rule inside the body must survive");
        assert!(p.body.contains("After the rule."));
        assert!(!p.body.contains("name: Lapis"));
    }

    #[test]
    fn no_frontmatter_is_valid_and_empty() {
        let p = parse("# Plain\n\nbody\n");
        assert!(p.hal_valid);
        assert!(p.hal.is_empty());
        assert_eq!(p.matter, None);
        assert_eq!(p.body, "# Plain\n\nbody");
    }

    #[test]
    fn invalid_yaml_sets_hal_valid_false_and_returns_body() {
        let p = parse("---\nname: [unclosed\nstatus: x\n---\nbody text\n");
        assert!(!p.hal_valid);
        assert!(p.hal.is_empty());
        assert!(p.error.is_some());
        assert_eq!(p.body, "body text");
        assert!(p.matter.as_deref().unwrap().starts_with("name: [unclosed"));
    }

    #[test]
    fn scalar_frontmatter_is_invalid() {
        let p = parse("---\njust a string\n---\nbody\n");
        assert!(!p.hal_valid);
        assert_eq!(p.body, "body");
    }

    #[test]
    fn empty_frontmatter_block_is_valid_empty() {
        let p = parse("---\n---\nbody\n");
        assert!(p.hal_valid);
        assert!(p.hal.is_empty());
        assert_eq!(p.body, "body");
    }

    #[test]
    fn unknown_keys_and_nested_values_preserved() {
        let p = parse("---\nname: n\nkarakeep-id: abc\nnested:\n  a: 1\n  b: [true, null]\n---\n");
        assert_eq!(p.hal["karakeep-id"], "abc");
        assert_eq!(p.hal["nested"], serde_json::json!({"a": 1, "b": [true, null]}));
    }

    #[test]
    fn title_prefers_name_then_title() {
        let p = parse("---\ntitle: T\nname: N\n---\n");
        assert_eq!(title_from_hal(&p.hal).as_deref(), Some("N"));
        let p = parse("---\ntitle: T\n---\n");
        assert_eq!(title_from_hal(&p.hal).as_deref(), Some("T"));
        let p = parse("---\nname: \"  \"\n---\n");
        assert_eq!(title_from_hal(&p.hal), None);
    }

    #[test]
    fn tags_array_or_csv() {
        let p = parse("---\ntags: [a, \"#b\", 3]\n---\n");
        assert_eq!(tags_from_hal(&p.hal), ["a", "b", "3"]);
        let p = parse("---\ntags: \"x, #y ,,z\"\n---\n");
        assert_eq!(tags_from_hal(&p.hal), ["x", "y", "z"]);
        let p = parse("---\nname: n\n---\n");
        assert!(tags_from_hal(&p.hal).is_empty());
    }

    #[test]
    fn non_string_keys_are_stringified() {
        let p = parse("---\n1: one\ntrue: yes\n---\n");
        assert!(p.hal_valid);
        assert_eq!(p.hal["1"], "one");
        assert_eq!(p.hal["true"], "yes");
    }
}
