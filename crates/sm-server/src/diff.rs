//! Minimal JSON diff producing path-based set/delete operations.

use serde_json::{json, Value};

fn rec(path: &mut Vec<Value>, old: &Value, new: &Value, ops: &mut Vec<Value>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            for (k, nv) in n {
                path.push(Value::from(k.as_str()));
                match o.get(k) {
                    Some(ov) => rec(path, ov, nv, ops),
                    None => ops.push(json!({ "p": path, "v": nv })),
                }
                path.pop();
            }
            for k in o.keys() {
                if !n.contains_key(k) {
                    path.push(Value::from(k.as_str()));
                    ops.push(json!({ "p": path, "d": true }));
                    path.pop();
                }
            }
        }
        // Same-length arrays are diffed element-wise (cheap for a cue edit in a
        // long list); anything else is replaced as a whole.
        (Value::Array(o), Value::Array(n)) if o.len() == n.len() => {
            for (i, (ov, nv)) in o.iter().zip(n).enumerate() {
                path.push(Value::from(i));
                rec(path, ov, nv, ops);
                path.pop();
            }
        }
        _ => ops.push(json!({ "p": path, "v": new })),
    }
}

/// Operations that transform `old` into `new`.
pub fn diff(old: &Value, new: &Value) -> Vec<Value> {
    let mut ops = Vec::new();
    rec(&mut Vec::new(), old, new, &mut ops);
    ops
}

/// Apply operations (used by tests and the Rust-side client helpers).
pub fn apply(doc: &mut Value, ops: &[Value]) {
    for op in ops {
        let Some(path) = op.get("p").and_then(Value::as_array) else {
            continue;
        };
        if path.is_empty() {
            if let Some(v) = op.get("v") {
                *doc = v.clone();
            }
            continue;
        }
        let mut cur = &mut *doc;
        for seg in &path[..path.len() - 1] {
            let next = match seg {
                Value::String(k) => cur.get_mut(k.as_str()),
                Value::Number(i) => i.as_u64().and_then(|i| cur.get_mut(i as usize)),
                _ => None,
            };
            match next {
                Some(n) => cur = n,
                None => return,
            }
        }
        let last = &path[path.len() - 1];
        let delete = op.get("d").and_then(Value::as_bool).unwrap_or(false);
        match (last, cur) {
            (Value::String(k), Value::Object(map)) => {
                if delete {
                    map.remove(k);
                } else if let Some(v) = op.get("v") {
                    map.insert(k.clone(), v.clone());
                }
            }
            (Value::Number(i), Value::Array(arr)) => {
                if let (Some(i), Some(v)) = (i.as_u64(), op.get("v")) {
                    if let Some(slot) = arr.get_mut(i as usize) {
                        *slot = v.clone();
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_then_apply_reproduces_target() {
        let old =
            json!({ "a": 1, "b": { "c": [1, 2, { "x": 1 }], "gone": true }, "list": [1, 2, 3] });
        let new = json!({ "a": 2, "b": { "c": [1, 2, { "x": 5, "y": null }], "added": "s" }, "list": [1, 2], "n": {} });
        let ops = diff(&old, &new);
        let mut doc = old.clone();
        apply(&mut doc, &ops);
        assert_eq!(doc, new);
        assert!(diff(&new, &new).is_empty());
        // element-wise array diff keeps patches small
        let ops = diff(
            &json!({ "l": [{ "n": 1 }, { "n": 2 }] }),
            &json!({ "l": [{ "n": 1 }, { "n": 3 }] }),
        );
        assert_eq!(ops, vec![json!({ "p": ["l", 1, "n"], "v": 3 })]);
    }
}
