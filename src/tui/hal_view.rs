//! HAL inspector (`Space y`): the well-known keys first, then the whole
//! mapping as YAML. Pure functions so the summary is testable without a frame.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

use super::theme;

/// Keys shown as the headline block, in this order, when present.
pub const SUMMARY_KEYS: [&str; 8] =
    ["name", "type", "doc_type", "domain", "status", "priority", "created", "updated"];

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `(key, value)` for every summary key present in the mapping.
pub fn summary(hal: &Map<String, Value>) -> Vec<(&'static str, String)> {
    SUMMARY_KEYS.iter().filter_map(|k| hal.get(*k).map(|v| (*k, scalar(v)))).collect()
}

/// Lines for the inspector pane.
pub fn lines(hal: &Map<String, Value>, valid: bool) -> Vec<Line<'static>> {
    if !valid {
        return vec![Line::from(Span::styled(
            "frontmatter is not valid YAML",
            Style::default().fg(theme::WARN),
        ))];
    }
    let mut out: Vec<Line<'static>> = summary(hal)
        .into_iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!("{k:<9}"), theme::accent()), Span::raw(v)]))
        .collect();
    if hal.is_empty() {
        out.push(Line::from(Span::styled("(no frontmatter)", theme::dim())));
        return out;
    }
    out.push(Line::default());
    let yaml = crate::write::to_yaml(hal).unwrap_or_default();
    out.extend(yaml.lines().map(|l| Line::from(Span::styled(l.to_string(), theme::dim()))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB: &str = "---\nname: Lapis · TUI parity\ntype: foundry-doc\ndomain: foundry\nstatus: draft\nhal_version: \"1.0\"\ntags: [tui, ratatui]\ncross_refs:\n  - \"[[SPEC]]\"\n---\n# body\n";

    #[test]
    fn summary_maps_type_domain_status() {
        let p = crate::hal::parse(BLOB);
        assert!(p.hal_valid);
        let s = summary(&p.hal);
        assert_eq!(
            s,
            [
                ("name", "Lapis · TUI parity".to_string()),
                ("type", "foundry-doc".to_string()),
                ("domain", "foundry".to_string()),
                ("status", "draft".to_string()),
            ]
        );
        let text: Vec<String> = lines(&p.hal, true).iter().map(|l| l.to_string()).collect();
        assert!(text[1].starts_with("type") && text[1].ends_with("foundry-doc"));
        assert!(text[2].starts_with("domain") && text[2].ends_with("foundry"));
        assert!(text[3].starts_with("status") && text[3].ends_with("draft"));
        // the raw YAML tail keeps keys the summary does not know
        assert!(text.iter().any(|l| l.contains("hal_version")));
    }

    #[test]
    fn invalid_and_empty() {
        let l = lines(&Map::new(), false);
        assert_eq!(l[0].to_string(), "frontmatter is not valid YAML");
        let l = lines(&Map::new(), true);
        assert_eq!(l[0].to_string(), "(no frontmatter)");
    }
}
