//! URLs as they are shown: in errors, `doctor` output and JSON. A configured
//! address may carry a password (`http://user:pass@host`) or a token in its
//! query; neither belongs in a message, a log or a cache file.

/// `u` without userinfo, query or fragment: `http://user:pw@host:1/p?k=v#f`
/// becomes `http://host:1/p`. Text that is not a URL is returned trimmed of the
/// same parts.
pub fn url(u: &str) -> String {
    let cut = u.find(['?', '#']).map_or(u, |i| &u[..i]);
    text(cut)
}

/// `s` with the userinfo dropped from every `scheme://user:pass@` in it. HTTP
/// client errors quote the URL they tried, so their text needs this too.
pub fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let end =
            tail.find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace() || c == ')' || c == '"');
        let authority = &tail[..end.unwrap_or(tail.len())];
        let host_from = authority.rfind('@').map_or(0, |at| at + 1);
        out.push_str(&tail[host_from..authority.len()]);
        rest = &tail[authority.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_and_queries_never_show() {
        assert_eq!(url("http://user:pw@host:11434/api/embed?key=x#f"), "http://host:11434/api/embed");
        assert_eq!(url("http://127.0.0.1:11434"), "http://127.0.0.1:11434");
        assert_eq!(url("https://token@example.test/"), "https://example.test/");
        assert_eq!(
            text("error sending request for url (http://a:b@h:1/x): refused; retry http://h2/y"),
            "error sending request for url (http://h:1/x): refused; retry http://h2/y"
        );
        assert_eq!(text("no url here"), "no url here");
        assert_eq!(text("weird ://@ tail"), "weird :// tail");
    }
}
