//! Inbound Phantom envelope (G3), parsed with local serde types.
//!
//! Shape from Phantom SPEC §9 and the G1 proto at Hedronite/phantom `2063a55`:
//! `{ok, data, error: {kind, message, exit} | null, meta}`. Unknown fields are
//! ignored on every object (no `deny_unknown_fields`), as §9 promises they are
//! additive. This is not `crate::envelope`, which is Lapis's own outbound shape.
//!
//! `error.kind` is kept as Phantom sent it; Lapis does not invent kinds. Only
//! `error.exit` is checked, against Phantom's canon 0 through 6.
//!
//! Nothing here opens a path. A string in `data` that looks like a file or a
//! database is a value to display, never something to read (T1).

use serde::Deserialize;
use serde_json::Value;

/// Most values kept from a `data` array (T2). The rest are dropped and the
/// reply is marked truncated; nothing spawns again to fetch them.
pub const DATA_CAP: usize = 256;

/// Phantom's exits: 0 ok, 1 usage or internal, 2 no index, 3 not found,
/// 4 writer lock, 5 schema newer, 6 ambiguous.
pub const EXITS: std::ops::RangeInclusive<i64> = 0..=6;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<EnvelopeError>,
    #[serde(default)]
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EnvelopeError {
    pub kind: String,
    pub message: String,
    pub exit: i64,
}

/// The `meta` fields Lapis reads. `next` is never followed (no second spawn).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub truncated: bool,
}

/// A parsed envelope whose `data` array is at most [`DATA_CAP`] long.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub envelope: Envelope,
    /// Lapis dropped values past [`DATA_CAP`], or Phantom said `meta.truncated`.
    pub truncated: bool,
}

impl Reply {
    /// Display lines for the desktop overlay: one per `data` array value (a
    /// string as is, anything else as compact JSON), one for any other `data`,
    /// and one for `error`. Text only; nothing named in a row is opened.
    pub fn rows(&self) -> Vec<String> {
        let text = |v: &Value| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let mut rows: Vec<String> = match &self.envelope.data {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().map(text).collect(),
            Some(other) => vec![text(other)],
        };
        if let Some(e) = &self.envelope.error {
            rows.push(format!("exit {} {}: {}", e.exit, e.kind, e.message));
        }
        rows
    }
}

/// Why stdout did not become a [`Reply`].
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Not JSON, or not the envelope shape.
    NotEnvelope(String),
    /// `ok` is false and there is no `error`.
    MissingError,
    /// `error.exit` is outside 0 through 6.
    UnknownExit(i64),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NotEnvelope(m) => write!(f, "not a Phantom envelope: {m}"),
            ParseError::MissingError => f.write_str("Phantom envelope has ok=false and no error"),
            ParseError::UnknownExit(n) => write!(f, "Phantom error.exit {n} is outside 0-6"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse one envelope from `stdout` and cap its `data` array.
pub fn parse(stdout: &[u8]) -> Result<Reply, ParseError> {
    let mut envelope: Envelope =
        serde_json::from_slice(stdout).map_err(|e| ParseError::NotEnvelope(e.to_string()))?;
    match &envelope.error {
        Some(e) if !EXITS.contains(&e.exit) => return Err(ParseError::UnknownExit(e.exit)),
        None if !envelope.ok => return Err(ParseError::MissingError),
        _ => {}
    }
    let capped = cap(&mut envelope.data);
    let truncated = capped || envelope.meta.as_ref().is_some_and(|m| m.truncated);
    Ok(Reply { envelope, truncated })
}

/// Keep the first [`DATA_CAP`] values of an array. True when any were dropped.
fn cap(data: &mut Option<Value>) -> bool {
    match data {
        Some(Value::Array(items)) if items.len() > DATA_CAP => {
            items.truncate(DATA_CAP);
            true
        }
        _ => false,
    }
}
