//! `.agents/skills` is the canonical tree. Harness copies are regular files
//! with the same bytes. `scripts/sync-skills` rewrites the copies.

use std::fs;
use std::path::{Path, PathBuf};

const COPIES: &[&str] = &[".claude/skills", ".grok/skills", ".cursor/skills"];
const ABSENT: &[&str] = &["skills", ".omp/skills", ".codex/skills"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn harness_skill_copies_match_canonical() {
    let root = root();
    let canon = root.join(".agents/skills");
    let files = regular_files(&canon);
    assert!(!files.is_empty(), "canonical skill tree is empty");
    for rel in &files {
        let expected = fs::read(canon.join(rel)).unwrap_or_else(|err| panic!("{rel:?}: {err}"));
        for copy in COPIES {
            let path = root.join(copy).join(rel);
            assert_regular_file(&path);
            let actual = fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            assert_eq!(actual, expected, "{}", path.display());
        }
    }
    for copy in COPIES {
        assert_eq!(regular_files(&root.join(copy)), files, "{copy}");
    }
    for absent in ABSENT {
        assert!(!root.join(absent).exists(), "{absent} must not exist");
    }

    let claude = fs::read(root.join("CLAUDE.md")).expect("CLAUDE.md");
    assert_eq!(claude, b"@AGENTS.md\n");

    let skill = fs::read_to_string(canon.join("lapis/SKILL.md")).expect("lapis SKILL.md");
    assert_eq!(frontmatter_keys(&skill), vec!["description".to_string(), "name".to_string()]);
    assert_eq!(frontmatter_value(&skill, "name"), "lapis");
    assert!(!frontmatter_value(&skill, "description").is_empty());
}

/// SKILL.md lists each wrapper, and `lapis.nu` defines it. The copies are
/// checked above, so this only reads the canonical tree.
#[test]
fn skill_lists_every_wrapper() {
    let root = root().join(".agents/skills/lapis");
    let skill = fs::read_to_string(root.join("SKILL.md")).unwrap();
    let body = fs::read_to_string(root.join("lapis.nu")).unwrap();
    for (listed, defined) in [
        ("nu lapis.nu search ", "def \"main search\""),
        ("nu lapis.nu read ", "def \"main read\""),
        ("nu lapis.nu list ", "def \"main list\""),
        ("nu lapis.nu neighbors ", "def \"main neighbors\""),
        ("nu lapis.nu resolve ", "def \"main resolve\""),
        ("nu lapis.nu recent ", "def \"main recent\""),
        ("nu lapis.nu analytics ", "def \"main analytics\""),
        ("nu lapis.nu tree-retrieve ", "def \"main tree-retrieve\""),
        ("nu lapis.nu status", "def \"main status\""),
        ("nu lapis.nu doctor", "def \"main doctor\""),
        ("nu lapis.nu create ", "def \"main create\""),
        ("nu lapis.nu append ", "def \"main append\""),
        ("nu lapis.nu task list ", "def \"main task list\""),
        ("nu lapis.nu task toggle ", "def \"main task toggle\""),
        ("nu lapis.nu tome tree ", "def \"main tome tree\""),
        ("nu lapis.nu tome open ", "def \"main tome open\""),
    ] {
        assert!(skill.contains(listed), "SKILL.md missing {listed}");
        assert!(body.contains(defined), "lapis.nu missing {defined}");
    }
    assert!(body.contains("^lapis --json"), "wrappers must pass --json");
    assert!(body.contains("\"--dry-run\""), "write wrappers must forward --dry-run");
    assert!(skill.contains("--features tome"));
}

fn assert_regular_file(path: &Path) {
    let meta = fs::symlink_metadata(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(meta.file_type().is_file(), "{} must be a regular file", path.display());
}

fn regular_files(dir: &Path) -> Vec<PathBuf> {
    assert_real_dir(dir);
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn assert_real_dir(path: &Path) {
    let meta = fs::symlink_metadata(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert!(meta.file_type().is_dir(), "{} must be a real directory", path.display());
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let kind = meta.file_type();
        assert!(!kind.is_symlink(), "{} is a symlink", path.display());
        if kind.is_dir() {
            walk(root, &path, out);
        } else if kind.is_file() {
            let rel = path.strip_prefix(root).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            out.push(rel.to_path_buf());
        } else {
            panic!("{} is not a regular file", path.display());
        }
    }
}

fn frontmatter_keys(md: &str) -> Vec<String> {
    let mut keys: Vec<String> = frontmatter(md)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with([' ', '\t']))
        .map(|line| line.split_once(':').expect("frontmatter key").0.to_string())
        .collect();
    keys.sort();
    keys
}

fn frontmatter_value(md: &str, key: &str) -> String {
    let yaml = frontmatter(md);
    let mut value = String::new();
    let mut taking = false;
    for line in yaml.lines() {
        if line.starts_with([' ', '\t']) {
            if taking {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.trim());
            }
            continue;
        }
        let (found, rest) = line.split_once(':').expect("frontmatter key");
        taking = found == key;
        if taking {
            value = rest.trim().trim_matches(|c| c == '"' || c == '\'' || c == '>').trim().to_string();
        }
    }
    value
}

fn frontmatter(md: &str) -> &str {
    let rest = md.strip_prefix("---\n").expect("opening ---");
    rest.split_once("\n---").expect("closing ---").0
}
