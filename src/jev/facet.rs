//! Facet batch request. Untrusted PDF text and the query sit in the JSON body.
//! `{` and `}` inside strings are `\u` escapes so a `{{typesafeApiKey}}` in
//! that text is not a placeholder Facet will fill in. The decoded string is
//! the original text. The Authorization header still uses the real placeholder.

use serde_json::Value;

pub(super) fn json(value: &Value) -> String {
    let marked = mark_template_braces(value);
    serde_json::to_string(&marked)
        .unwrap_or_else(|_| "{}".into())
        .replace('\u{E000}', "\\u007b")
        .replace('\u{E001}', "\\u007d")
}

fn mark_template_braces(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let mut out = String::with_capacity(text.len());
            for ch in text.chars() {
                match ch {
                    '{' => out.push('\u{E000}'),
                    '}' => out.push('\u{E001}'),
                    _ => out.push(ch),
                }
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(mark_template_braces).collect()),
        Value::Object(map) => {
            let mut next = serde_json::Map::new();
            for (key, child) in map {
                next.insert(key.clone(), mark_template_braces(child));
            }
            Value::Object(next)
        }
        other => other.clone(),
    }
}

/// Facet collection whose body is one batch System One request. The hit-rerank
/// recipe stays on the single relevance questions; a batch must not use it.
pub(super) fn collection(body: &str) -> String {
    format!(
        "\
opencollection: 1.0.0
info:
  name: Tome batch
  version: 0.0.0
config:
  environments:
    - name: typesafe
      variables:
        - name: typesafeApi
          value: https://api.typesafe.ai
        - secret: true
          name: typesafeApiKey
          type: string
items:
  - info:
      name: System One
      type: folder
      seq: 1
    items:
      - info:
          name: Batch
          type: http
          seq: 1
        http:
          method: POST
          url: \"{{{{typesafeApi}}}}/v1/systemone\"
          headers:
            - name: Authorization
              value: \"Bearer {{{{typesafeApiKey}}}}\"
            - name: Content-Type
              value: application/json
            - name: Accept
              value: application/json
          body:
            type: json
            data: |-
              {body}
"
    )
}
