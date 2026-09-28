//! One shared answerer for both arms: same prompt, same model, same budget.
//! The model id, agent and temperature come from `config.toml` (never from code).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use crate::config::AnswerCfg;

/// A passage handed to the answerer, labelled with its physical page span.
#[derive(Debug, Clone, PartialEq)]
pub struct PassageIn {
    /// `chunk 90864` or `node 0003.0002`.
    pub label: String,
    pub page_start: u32,
    pub page_end: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnswerOut {
    pub answer: String,
    pub cited_pages: Vec<u32>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub estimated: bool,
    pub latency_ms: f64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AnswerError {
    #[error("answer model failed: {0}")]
    Model(String),
    #[error("answer did not parse as {{answer, cited_pages}}: {0}")]
    Parse(String),
    #[error("answer model timed out after {0}s")]
    Timeout(u64),
}

impl AnswerError {
    pub fn kind(&self) -> &'static str {
        match self {
            AnswerError::Model(_) => "answer_model",
            AnswerError::Parse(_) => "answer_parse",
            AnswerError::Timeout(_) => "timeout",
        }
    }
}

/// Drop trailing (lowest-ranked) passages until the total fits `max_chars`.
/// Returns the kept passages and whether anything was dropped.
pub fn fit_budget(passages: Vec<PassageIn>, max_chars: usize) -> (Vec<PassageIn>, bool) {
    let mut used = 0usize;
    let mut kept = Vec::new();
    let total = passages.len();
    for p in passages {
        if used + p.text.len() > max_chars {
            break;
        }
        used += p.text.len();
        kept.push(p);
    }
    let dropped = kept.len() < total;
    (kept, dropped)
}

pub fn build_prompt(
    doc_title: &str,
    question: &str,
    passages: &[PassageIn],
    max_output_tokens: u32,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Answer a question about the PDF \"{doc_title}\" using ONLY the passages below.\n\
         Each passage is labelled with its 1-based physical PDF page number(s).\n\
         Reply with exactly one JSON object and nothing else:\n\
         {{\"answer\": \"<concise answer>\", \"cited_pages\": [<page numbers that support the answer>]}}\n\
         If the passages do not contain the answer, reply {{\"answer\": \"NOT_FOUND\", \"cited_pages\": []}}.\n\
         Keep the answer under {max_output_tokens} tokens.\n\n"
    ));
    for p in passages {
        let pages = if p.page_start == p.page_end {
            format!("page {}", p.page_start)
        } else {
            format!("pages {}-{}", p.page_start, p.page_end)
        };
        s.push_str(&format!("[{} | {pages}]\n{}\n\n", p.label, p.text.trim()));
    }
    s.push_str(&format!("Question: {question}\n"));
    s
}

#[derive(Deserialize)]
struct Reply {
    answer: String,
    #[serde(default)]
    cited_pages: Vec<Value>,
}

/// Parse `{answer, cited_pages}` from model text (tolerates code fences / chatter).
pub fn parse_reply(text: &str) -> Result<(String, Vec<u32>), AnswerError> {
    let start = text.find('{').ok_or_else(|| AnswerError::Parse(clip(text)))?;
    let end = text.rfind('}').ok_or_else(|| AnswerError::Parse(clip(text)))?;
    if end < start {
        return Err(AnswerError::Parse(clip(text)));
    }
    let r: Reply = serde_json::from_str(&text[start..=end])
        .map_err(|e| AnswerError::Parse(format!("{e}: {}", clip(text))))?;
    let mut pages: Vec<u32> = r
        .cited_pages
        .iter()
        .filter_map(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())))
        .filter(|p| *p >= 1 && *p <= u32::MAX as u64)
        .map(|p| p as u32)
        .collect();
    pages.sort_unstable();
    pages.dedup();
    Ok((r.answer, pages))
}

fn clip(s: &str) -> String {
    s.chars().take(200).collect()
}

pub enum Answerer {
    OpenCode {
        cfg: AnswerCfg,
        workdir: PathBuf,
    },
    /// Fixed reply; for tests and `--dry-answer`.
    Fake {
        reply: String,
    },
}

impl Answerer {
    pub fn from_config(cfg: &AnswerCfg) -> Result<Self, String> {
        match cfg.provider.as_str() {
            "opencode" => {
                let workdir = std::env::temp_dir().join("tome-eval-opencode");
                write_agent(&workdir, cfg)?;
                Ok(Answerer::OpenCode { cfg: cfg.clone(), workdir })
            }
            "fake" => Ok(Answerer::Fake { reply: r#"{"answer":"NOT_FOUND","cited_pages":[]}"#.into() }),
            other => Err(format!("answer.provider must be opencode|fake, got {other}")),
        }
    }

    pub async fn answer(&self, prompt: &str) -> Result<AnswerOut, AnswerError> {
        let t0 = Instant::now();
        match self {
            Answerer::Fake { reply } => {
                let (answer, cited_pages) = parse_reply(reply)?;
                Ok(AnswerOut {
                    answer,
                    cited_pages,
                    prompt_tokens: (prompt.len() / 4) as u64,
                    completion_tokens: (reply.len() / 4) as u64,
                    estimated: true,
                    latency_ms: t0.elapsed().as_secs_f64() * 1000.0,
                })
            }
            Answerer::OpenCode { cfg, workdir } => {
                let (text, session) = opencode_run(cfg, workdir, prompt).await?;
                let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
                let (answer, cited_pages) = parse_reply(&text)?;
                let usage = match &session {
                    Some(sid) => opencode_usage(cfg, workdir, sid).await,
                    None => None,
                };
                let (prompt_tokens, completion_tokens, estimated) = match usage {
                    Some((p, c)) => (p, c, false),
                    None => ((prompt.len() / 4) as u64, (text.len() / 4) as u64, true),
                };
                Ok(AnswerOut { answer, cited_pages, prompt_tokens, completion_tokens, estimated, latency_ms })
            }
        }
    }
}

/// OpenCode project agent that pins temperature and disables tools. Written from config
/// so `config.toml` stays the single source of truth for the answer settings.
pub fn agent_markdown(cfg: &AnswerCfg) -> String {
    format!(
        "---\ndescription: tome-eval shared answerer (generated from evals/tome/config.toml). Passages only, no tools.\n\
         mode: primary\ntemperature: {}\ntools:\n  \"*\": false\npermission:\n  \"*\": deny\n---\n\
         You answer questions strictly from the passages in the user message. Reply with one JSON object and nothing else.\n",
        cfg.temperature
    )
}

fn write_agent(workdir: &Path, cfg: &AnswerCfg) -> Result<(), String> {
    let dir = workdir.join(".opencode").join("agent");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(format!("{}.md", cfg.agent));
    std::fs::write(&file, agent_markdown(cfg)).map_err(|e| format!("{}: {e}", file.display()))?;
    // OpenCode resolves the project from the git root; a bare repo keeps it local.
    if !workdir.join(".git").exists() {
        let _ = std::process::Command::new("git").args(["init", "-q"]).current_dir(workdir).status();
    }
    Ok(())
}

async fn opencode_run(
    cfg: &AnswerCfg,
    workdir: &Path,
    prompt: &str,
) -> Result<(String, Option<String>), AnswerError> {
    let child = tokio::process::Command::new(&cfg.opencode_bin)
        .args(["run", "--standalone", "--agent", &cfg.agent, "--model", &cfg.model, "--format", "json"])
        .arg(prompt)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AnswerError::Model(format!("spawn {}: {e}", cfg.opencode_bin)))?;
    let out = tokio::time::timeout(Duration::from_secs(cfg.timeout_s), child.wait_with_output())
        .await
        .map_err(|_| AnswerError::Timeout(cfg.timeout_s))?
        .map_err(|e| AnswerError::Model(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (text, session) = parse_run_events(&stdout);
    if text.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AnswerError::Model(format!(
            "exit {}; no text part; {}",
            out.status,
            clip(&format!("{err}{stdout}"))
        )));
    }
    Ok((text, session))
}

/// `opencode run --format json` emits one event per line; text lives in `part.text`.
pub fn parse_run_events(stdout: &str) -> (String, Option<String>) {
    let mut text = String::new();
    let mut session = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if session.is_none() {
            session = v.get("sessionID").and_then(Value::as_str).map(str::to_string);
        }
        if v.get("type").and_then(Value::as_str) == Some("text")
            && let Some(t) = v.pointer("/part/text").and_then(Value::as_str)
        {
            text.push_str(t);
        }
    }
    (text, session)
}

async fn opencode_usage(cfg: &AnswerCfg, workdir: &Path, session: &str) -> Option<(u64, u64)> {
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&cfg.opencode_bin)
            .args(["session", "export", session])
            .current_dir(workdir)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    usage_from_export(&v)
}

/// Sum assistant-message usage (the session total also counts title generation).
pub fn usage_from_export(v: &Value) -> Option<(u64, u64)> {
    let msgs = v.get("messages")?.as_array()?;
    let mut p = 0u64;
    let mut c = 0u64;
    let mut any = false;
    for m in msgs.iter().filter(|m| m.get("type").and_then(Value::as_str) == Some("assistant")) {
        let t = m.get("tokens")?;
        let n = |ptr: &str| t.pointer(ptr).and_then(Value::as_u64).unwrap_or(0);
        p += n("/input") + n("/cache/read") + n("/cache/write");
        c += n("/output") + n("/reasoning");
        any = true;
    }
    any.then_some((p, c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> AnswerCfg {
        AnswerCfg {
            provider: "opencode".into(),
            model: "provider/some-model".into(),
            agent: "tome-answerer".into(),
            temperature: 0.0,
            max_output_tokens: 400,
            opencode_bin: "opencode".into(),
            timeout_s: 5,
        }
    }

    #[test]
    fn reply_parses_through_fences_and_string_pages() {
        let (a, p) = parse_reply("```json\n{\"answer\":\"x\",\"cited_pages\":[\"87\",86,86]}\n```").unwrap();
        assert_eq!(a, "x");
        assert_eq!(p, vec![86, 87]);
        assert_eq!(parse_reply("no json").unwrap_err().kind(), "answer_parse");
    }

    #[test]
    fn budget_drops_tail_passages_only() {
        let mk =
            |n: usize| PassageIn { label: format!("c{n}"), page_start: 1, page_end: 1, text: "x".repeat(10) };
        let (kept, dropped) = fit_budget(vec![mk(1), mk(2), mk(3)], 25);
        assert_eq!(kept.len(), 2);
        assert!(dropped);
        assert_eq!(kept[0].label, "c1");
    }

    #[test]
    fn prompt_labels_pages_and_ends_with_question() {
        let p = build_prompt(
            "SRE",
            "What?",
            &[PassageIn { label: "chunk 1".into(), page_start: 86, page_end: 87, text: "body".into() }],
            300,
        );
        assert!(p.contains("[chunk 1 | pages 86-87]"));
        assert!(p.trim_end().ends_with("Question: What?"));
    }

    #[test]
    fn agent_file_pins_temperature_from_config() {
        let md = agent_markdown(&cfg());
        assert!(md.contains("temperature: 0"));
        assert!(md.contains("\"*\": false"));
        assert!(!md.contains("model:"), "model is passed per call from config");
    }

    #[test]
    fn run_events_and_export_usage() {
        let out = concat!(
            r#"{"type":"step_start","sessionID":"ses_1","part":{"type":"step-start"}}"#,
            "\n",
            r#"{"type":"text","sessionID":"ses_1","part":{"type":"text","text":"{\"answer\":\"ok\",\"cited_pages\":[3]}"}}"#
        );
        let (t, s) = parse_run_events(out);
        assert_eq!(s.as_deref(), Some("ses_1"));
        assert!(t.contains("\"ok\""));
        let v = json!({"messages":[
            {"type":"user"},
            {"type":"assistant","tokens":{"input":322,"output":12,"reasoning":0,"cache":{"read":10,"write":0}}}
        ]});
        assert_eq!(usage_from_export(&v), Some((332, 12)));
    }

    #[tokio::test]
    async fn fake_answerer_estimates_tokens() {
        let a = Answerer::Fake { reply: r#"{"answer":"a","cited_pages":[5]}"#.into() };
        let o = a.answer("prompt text").await.unwrap();
        assert_eq!(o.cited_pages, vec![5]);
        assert!(o.estimated);
    }
}
