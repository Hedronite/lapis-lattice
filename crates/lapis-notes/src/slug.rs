//! Slugging a title into a filename stem, unchanged from the CLI.
/// Lowercase Unicode slug for a filename stem: alphanumeric characters are
/// kept (lowercased), every other run becomes one `-`; the stem is never empty.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "note".to_string() } else { out }
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slug_rules() {
        assert_eq!(slug("Ship the Golden Note"), "ship-the-golden-note");
        assert_eq!(slug("Inbox drop: Café & Ünïcode!"), "inbox-drop-café-ünïcode");
        assert_eq!(slug("---"), "note");
        assert_eq!(slug("  "), "note");
        assert_eq!(slug("a  b__c"), "a-b-c");
        assert_eq!(slug("TRAILING "), "trailing");
    }
}
