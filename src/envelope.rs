//! The `--json` envelope every command prints (and MCP returns as
//! `structuredContent`): `{ok, data, error, meta}`. `meta` always carries
//! `latency_ms`, `truncated`, `next`; paged and search results add more.

use serde::Serialize;
use serde_json::Value;

use crate::error::LapisError;
use crate::lattice::Latency;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Meta {
    /// Wall time of this command / tool call, milliseconds.
    pub latency_ms: f64,
    /// `true` when `data` is a prefix of what matched (page full, body clipped).
    pub truncated: bool,
    /// Offset to pass back for the next page; `null` when there is none.
    pub next: Option<u32>,
    /// Search only: per-arm latency from the lattice (zeros when an arm was skipped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

impl Meta {
    /// Page bookkeeping: `truncated` when the page came back full.
    pub fn page(count: usize, limit: u32, offset: u32) -> Self {
        let truncated = limit > 0 && count as u32 >= limit;
        Meta {
            truncated,
            next: if truncated { Some(offset + limit) } else { None },
            count: Some(count),
            limit: Some(limit),
            offset: Some(offset),
            ..Meta::default()
        }
    }
    pub fn with_latency(mut self, ms: f64) -> Self {
        self.latency_ms = ms;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorBody {
    pub kind: &'static str,
    pub message: String,
    pub exit: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Envelope<T: Serialize> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<ErrorBody>,
    pub meta: Meta,
}

pub fn ok<T: Serialize>(data: T, meta: Meta) -> Envelope<T> {
    Envelope { ok: true, data: Some(data), error: None, meta }
}

pub fn err(e: &LapisError, latency_ms: f64) -> Envelope<Value> {
    Envelope {
        ok: false,
        data: None,
        error: Some(ErrorBody { kind: e.kind(), message: e.message().to_string(), exit: e.exit_code() }),
        meta: Meta { latency_ms, ..Meta::default() },
    }
}

/// Page a full in-memory list the way the lattice pages its tables.
pub fn slice_page<T: Clone>(all: &[T], limit: u32, offset: u32) -> (Vec<T>, Meta) {
    let start = (offset as usize).min(all.len());
    let end = if limit == 0 { all.len() } else { (start + limit as usize).min(all.len()) };
    let page = all[start..end].to_vec();
    let more = end < all.len();
    let meta = Meta {
        truncated: more,
        next: if more { Some(end as u32) } else { None },
        count: Some(page.len()),
        limit: Some(limit),
        offset: Some(offset),
        ..Meta::default()
    };
    (page, meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_and_err_shapes() {
        let v = serde_json::to_value(ok(vec![1, 2], Meta::default().with_latency(1.5))).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"], serde_json::json!([1, 2]));
        assert!(v["error"].is_null());
        assert_eq!(v["meta"]["latency_ms"], 1.5);
        assert_eq!(v["meta"]["truncated"], false);
        assert!(v["meta"].get("next").is_some() && v["meta"]["next"].is_null());
        assert!(v["meta"].get("latency").is_none());

        let e = serde_json::to_value(err(&LapisError::Path("not found: x.md".into()), 0.2)).unwrap();
        assert_eq!(e["ok"], false);
        assert!(e["data"].is_null());
        assert_eq!(e["error"]["kind"], "path");
        assert_eq!(e["error"]["exit"], 3);
        assert_eq!(e["error"]["message"], "not found: x.md");
        assert_eq!(e["meta"]["truncated"], false);
    }

    #[test]
    fn paging_meta() {
        let m = Meta::page(50, 50, 100);
        assert!(m.truncated);
        assert_eq!(m.next, Some(150));
        let m = Meta::page(12, 50, 100);
        assert!(!m.truncated && m.next.is_none());
        let all: Vec<u32> = (0..10).collect();
        let (p, m) = slice_page(&all, 4, 8);
        assert_eq!(p, [8, 9]);
        assert!(!m.truncated);
        let (p, m) = slice_page(&all, 4, 4);
        assert_eq!(p, [4, 5, 6, 7]);
        assert!(m.truncated);
        assert_eq!(m.next, Some(8));
        let (p, m) = slice_page(&all, 0, 0);
        assert_eq!(p.len(), 10);
        assert!(!m.truncated);
        let (p, _) = slice_page(&all, 5, 99);
        assert!(p.is_empty());
    }
}
