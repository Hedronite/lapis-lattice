//! Reading view input: `[[wikilinks]]` become Markdown links the reader can route back.
//! Code spans and fenced blocks are left exactly as written.

/// URL scheme for a rewritten wikilink. The reader's click handler strips it.
pub const SCHEME: &str = "lapis-wiki:";

/// `[[a/b|B]]` → `[B](<lapis-wiki:a/b>)`, `![[x]]` → `[x](<lapis-wiki:x>)`.
pub fn to_markdown_links(source: &str) -> String {
    if !source.contains("[[") {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len() + 64);
    let mut fence: Option<&str> = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(m)) => fence = Some(m),
            (Some(open), Some(m)) if open == m => fence = None,
            _ => {}
        }
        if fence.is_some() || marker.is_some() {
            out.push_str(line);
        } else {
            rewrite_line(line, &mut out);
        }
    }
    out
}

fn rewrite_line(line: &str, out: &mut String) {
    // Even-indexed pieces between backticks are prose; odd ones are inline code.
    for (i, piece) in line.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        if i % 2 == 1 {
            out.push_str(piece);
            continue;
        }
        let mut rest = piece;
        while let Some(start) = rest.find("[[") {
            let Some(len) = rest[start + 2..].find("]]") else { break };
            let embed = start > 0 && rest[..start].ends_with('!');
            out.push_str(&rest[..if embed { start - 1 } else { start }]);
            let inner = &rest[start + 2..start + 2 + len];
            let (target, alias) = inner.split_once('|').map_or((inner, None), |(t, a)| (t, Some(a)));
            let label = alias.unwrap_or(target).replace(['[', ']'], "");
            out.push_str(&format!("[{label}](<{SCHEME}{}>)", target.trim().replace(['<', '>'], "")));
            rest = &rest[start + 2 + len + 2..];
        }
        out.push_str(rest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_embeds_become_markdown_links() {
        assert_eq!(
            to_markdown_links("See [[a/b|B]] and ![[img.png]] and [[c d]].\n"),
            "See [B](<lapis-wiki:a/b>) and [img.png](<lapis-wiki:img.png>) and [c d](<lapis-wiki:c d>).\n"
        );
    }

    #[test]
    fn code_is_left_alone() {
        let source = "Use `[[x]]` here.\n```\n[[y]]\n```\nThen [[z]].\n";
        assert_eq!(
            to_markdown_links(source),
            "Use `[[x]]` here.\n```\n[[y]]\n```\nThen [z](<lapis-wiki:z>).\n"
        );
        assert_eq!(to_markdown_links("no links"), "no links");
        assert_eq!(to_markdown_links("open [[x and done"), "open [[x and done");
    }
}
