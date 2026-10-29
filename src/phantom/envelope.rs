//! Inbound Phantom envelope (G3), parsed with local serde types.
//!
//! Shape from Phantom SPEC §9 and Phantom's proto at `486e5f3`:
//! `{ok, data, error: {kind, message, exit} | null, schema}`. Unknown fields
//! are ignored on every object (no `deny_unknown_fields`), as §9 promises
//! they are additive; `meta` is one of them (G5g). Phantom says a list was
//! cut short with `truncated` inside `data` (`calls.rs:149-154` at
//! `486e5f3`), so that is where Lapis reads it. This is not
//! `crate::envelope`, which is Lapis's own outbound shape.
//!
//! `schema` is read first and alone (G5). Anything but [`PHANTOM_SCHEMA`] is
//! refused before `data` is looked at. There is no "accept newer": a schema
//! bump on Phantom is a breaking change, adopted by changing this constant
//! and the pin in `PHANTOM_PIN`. `data.store_schema` and
//! `data.extractor_version` describe Phantom's store, not the envelope, and
//! are never checked.
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

/// The one envelope `schema` Lapis supports: `SCHEMA_VERSION` in Phantom's
/// proto at the commit in `PHANTOM_PIN`.
pub const PHANTOM_SCHEMA: u64 = 1;

/// Phantom's exits, mirrored here to check `error.exit`: 0 ok, 1 usage or
/// internal, 2 no index, 3 not found, 4 writer lock, 5 schema newer (Phantom's
/// store), 6 ambiguous. They are Phantom's codes, never Lapis CLI exits.
pub const EXITS: std::ops::RangeInclusive<i64> = 0..=6;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<EnvelopeError>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EnvelopeError {
    pub kind: String,
    pub message: String,
    pub exit: i64,
}

/// A parsed envelope whose `data` array is at most [`DATA_CAP`] long.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub envelope: Envelope,
    /// Lapis dropped values past [`DATA_CAP`], or Phantom set `data.truncated`
    /// to `true`. `meta` is not read, so `meta.next` is never followed.
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
    /// `schema` is not [`PHANTOM_SCHEMA`]. Nothing past `schema` was used.
    Schema(Schema),
}

/// The `schema` a refused envelope carried. Only a number is kept, so the
/// refusal never repeats a path or any other content from the envelope.
#[derive(Debug, Clone, PartialEq)]
pub enum Schema {
    /// No `schema`, or `null`.
    Missing,
    /// A number other than [`PHANTOM_SCHEMA`]: `0`, `2`, `-1`, `1.5`.
    Number(serde_json::Number),
    /// A string, a boolean, an array or an object.
    NotANumber,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NotEnvelope(m) => write!(f, "not a Phantom envelope: {m}"),
            ParseError::MissingError => f.write_str("Phantom envelope has ok=false and no error"),
            ParseError::UnknownExit(n) => write!(f, "Phantom error.exit {n} is outside 0-6"),
            ParseError::Schema(Schema::Missing) => {
                write!(f, "Phantom envelope has no schema; Lapis supports schema {PHANTOM_SCHEMA}")
            }
            ParseError::Schema(Schema::Number(n)) => {
                write!(
                    f,
                    "Phantom envelope schema {n} is not supported; Lapis supports schema {PHANTOM_SCHEMA}"
                )
            }
            ParseError::Schema(Schema::NotANumber) => {
                write!(f, "Phantom envelope schema is not a number; Lapis supports schema {PHANTOM_SCHEMA}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse one envelope from `stdout` and cap its `data` array. `schema` is
/// checked first; a refused envelope is never deserialized further.
pub fn parse(stdout: &[u8]) -> Result<Reply, ParseError> {
    check_schema(stdout)?;
    let mut envelope: Envelope =
        serde_json::from_slice(stdout).map_err(|e| ParseError::NotEnvelope(e.to_string()))?;
    match &envelope.error {
        Some(e) if !EXITS.contains(&e.exit) => return Err(ParseError::UnknownExit(e.exit)),
        None if !envelope.ok => return Err(ParseError::MissingError),
        _ => {}
    }
    let capped = cap(&mut envelope.data);
    let said = envelope.data.as_ref().and_then(|d| d.get("truncated")) == Some(&Value::Bool(true));
    let truncated = capped || said;
    Ok(Reply { envelope, truncated })
}

/// G5: read `schema` alone. Every other field is skipped, not kept.
fn check_schema(stdout: &[u8]) -> Result<(), ParseError> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        schema: Option<Value>,
    }
    // A struct also deserializes from a JSON array; an envelope is an object.
    if stdout.trim_ascii_start().first() != Some(&b'{') {
        return Err(ParseError::NotEnvelope("expected a JSON object".into()));
    }
    let head: Head = serde_json::from_slice(stdout).map_err(|e| ParseError::NotEnvelope(e.to_string()))?;
    let refused = match head.schema {
        Some(Value::Number(n)) if n.as_u64() == Some(PHANTOM_SCHEMA) => return Ok(()),
        None | Some(Value::Null) => Schema::Missing,
        Some(Value::Number(n)) => Schema::Number(n),
        Some(_) => Schema::NotANumber,
    };
    Err(ParseError::Schema(refused))
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
