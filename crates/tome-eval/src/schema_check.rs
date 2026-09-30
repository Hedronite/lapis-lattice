//! Minimal JSON Schema (2020-12 subset) checker so results can be validated against
//! the committed `tome-eval-result.schema.json` without Python or a new crate.
//! Supports: $ref (#/$defs/...), oneOf, allOf with if/then on const, type (incl.
//! arrays of types), enum, const, required, properties, additionalProperties:false
//! or a schema, items, uniqueItems, minimum/maximum, pattern.

use serde_json::Value;

pub struct Checker<'a> {
    root: &'a Value,
}

impl<'a> Checker<'a> {
    pub fn new(root: &'a Value) -> Self {
        Self { root }
    }

    pub fn validate(&self, v: &Value) -> Vec<String> {
        let mut errs = Vec::new();
        self.check(self.root, v, "$", &mut errs);
        errs
    }

    fn resolve(&self, s: &'a Value) -> &'a Value {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let ptr = r.trim_start_matches('#');
                self.root.pointer(ptr).map(|t| self.resolve(t)).unwrap_or(s)
            }
            None => s,
        }
    }

    fn check(&self, schema: &'a Value, v: &Value, at: &str, errs: &mut Vec<String>) {
        let s = self.resolve(schema);
        if let Some(opts) = s.get("oneOf").and_then(Value::as_array) {
            let ok = opts
                .iter()
                .filter(|o| {
                    let mut e = Vec::new();
                    self.check(o, v, at, &mut e);
                    e.is_empty()
                })
                .count();
            if ok != 1 {
                let mut detail = Vec::new();
                for o in opts {
                    let mut e = Vec::new();
                    self.check(o, v, at, &mut e);
                    detail.push(e.into_iter().take(3).collect::<Vec<_>>().join("; "));
                }
                errs.push(format!("{at}: oneOf matched {ok} branches [{}]", detail.join(" | ")));
            }
        }
        if let Some(all) = s.get("allOf").and_then(Value::as_array) {
            for a in all {
                if let (Some(cond), Some(then)) = (a.get("if"), a.get("then")) {
                    let mut e = Vec::new();
                    self.check(cond, v, at, &mut e);
                    if e.is_empty() {
                        self.check(then, v, at, errs);
                    }
                } else {
                    self.check(a, v, at, errs);
                }
            }
        }
        if let Some(t) = s.get("type") {
            let types: Vec<&str> = match t {
                Value::String(x) => vec![x.as_str()],
                Value::Array(xs) => xs.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if !types.iter().any(|t| type_ok(t, v)) {
                errs.push(format!("{at}: expected {types:?}, got {}", kind(v)));
                return;
            }
        }
        if let Some(c) = s.get("const")
            && c != v
        {
            errs.push(format!("{at}: expected const {c}, got {v}"));
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array)
            && !e.contains(v)
        {
            errs.push(format!("{at}: {v} not in enum"));
        }
        if let Some(n) = v.as_f64() {
            if let Some(m) = s.get("minimum").and_then(Value::as_f64)
                && n < m
            {
                errs.push(format!("{at}: {n} < minimum {m}"));
            }
            if let Some(m) = s.get("maximum").and_then(Value::as_f64)
                && n > m
            {
                errs.push(format!("{at}: {n} > maximum {m}"));
            }
        }
        if let (Some(p), Some(x)) = (s.get("pattern").and_then(Value::as_str), v.as_str()) {
            match regex::Regex::new(p) {
                Ok(re) if !re.is_match(x) => errs.push(format!("{at}: {x:?} !~ /{p}/")),
                Err(e) => errs.push(format!("{at}: bad pattern {p}: {e}")),
                _ => {}
            }
        }
        if let Some(obj) = v.as_object() {
            if let Some(req) = s.get("required").and_then(Value::as_array) {
                for r in req.iter().filter_map(Value::as_str) {
                    if !obj.contains_key(r) {
                        errs.push(format!("{at}: missing required {r}"));
                    }
                }
            }
            let props = s.get("properties").and_then(Value::as_object);
            for (k, val) in obj {
                let path = format!("{at}.{k}");
                match props.and_then(|p| p.get(k)) {
                    Some(ps) => self.check(ps, val, &path, errs),
                    None => match s.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            errs.push(format!("{path}: additional property not allowed"))
                        }
                        Some(ap @ Value::Object(_)) => self.check(ap, val, &path, errs),
                        _ => {}
                    },
                }
            }
        }
        if let Some(arr) = v.as_array() {
            if let Some(items) = s.get("items") {
                for (i, it) in arr.iter().enumerate() {
                    self.check(items, it, &format!("{at}[{i}]"), errs);
                }
            }
            if s.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
                for (i, a) in arr.iter().enumerate() {
                    if arr[..i].contains(a) {
                        errs.push(format!("{at}[{i}]: duplicate item {a}"));
                    }
                }
            }
        }
    }
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => false,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn committed_schema() -> Value {
    serde_json::from_str(include_str!("../../../evals/tome/schema/tome-eval-result.schema.json"))
        .expect("committed schema is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checker_catches_the_basics() {
        let s = json!({"$defs":{"p":{"type":"integer","minimum":1}},"type":"object","additionalProperties":false,
            "required":["a"],"properties":{"a":{"$ref":"#/$defs/p"},"b":{"type":["string","null"],"enum":["x",null]}}});
        let c = Checker::new(&s);
        assert!(c.validate(&json!({"a":2,"b":null})).is_empty());
        assert!(!c.validate(&json!({"a":0})).is_empty());
        assert!(!c.validate(&json!({"b":"x"})).is_empty());
        assert!(!c.validate(&json!({"a":1,"z":1})).is_empty());
        assert!(!c.validate(&json!({"a":1,"b":"y"})).is_empty());
    }

    #[test]
    fn committed_schema_loads_and_has_both_record_types() {
        let s = committed_schema();
        assert!(s.pointer("/$defs/result").is_some());
        assert!(s.pointer("/$defs/summary").is_some());
        let kinds = s.pointer("/$defs/errorKind/enum").unwrap().as_array().unwrap();
        for k in ["over_budget", "no_structure", "empty", "judge_unavailable"] {
            assert!(kinds.contains(&json!(k)), "{k}");
        }
    }
}
