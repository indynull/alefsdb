use crate::ast::{CmpOp, Expr, Predicate, QueryHit};
use crate::parse::{parse_query, ParseError};
use alefs_namespace::{Database, EntryKind};
use alefs_storage::Storage;
use alefs_types::{DbPath, Scalar, Value};

#[derive(Debug)]
pub enum EvalError {
    Parse(ParseError),
    Ns(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Parse(e) => write!(f, "{e}"),
            EvalError::Ns(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EvalError {}

pub fn execute<S: Storage>(db: &Database<S>, query: &str) -> Result<Vec<QueryHit>, EvalError> {
    let expr = parse_query(query).map_err(EvalError::Parse)?;
    let candidates = candidates_for(db, &expr).map_err(|e| EvalError::Ns(e.to_string()))?;
    let mut hits = Vec::new();
    for (path, kind, value) in candidates {
        if eval_expr(&expr, &path, kind, value.as_ref()) {
            let type_name = match (&kind, &value) {
                (EntryKind::Directory, _) => "directory".into(),
                (EntryKind::Value, Some(v)) => type_label(v),
                (EntryKind::Value, None) => "value".into(),
            };
            hits.push(QueryHit { path, type_name });
        }
    }
    Ok(hits)
}

/// If the expression AND-chain implies a concrete type, use the type index.
fn required_concrete_type(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Pred(Predicate::TypeName(t)) if t != "scalar" => Some(t.as_str()),
        Expr::And(a, b) => match (required_concrete_type(a), required_concrete_type(b)) {
            (Some(t), None) | (None, Some(t)) => Some(t),
            (Some(t1), Some(t2)) if t1 == t2 => Some(t1),
            _ => None,
        },
        _ => None,
    }
}

fn candidates_for<S: Storage>(
    db: &Database<S>,
    expr: &Expr,
) -> Result<Vec<(DbPath, EntryKind, Option<Value>)>, alefs_namespace::NsError> {
    if let Some(ty) = required_concrete_type(expr) {
        let mut out = Vec::new();
        for path in db.paths_with_type(ty)? {
            let entry = db.get(&path)?;
            out.push((path, entry.kind, entry.value));
        }
        return Ok(out);
    }
    all_entries(db)
}

fn type_label(v: &Value) -> String {
    match v {
        Value::Scalar(Scalar::Null) => "null".into(),
        Value::Scalar(Scalar::Bool(_)) => "bool".into(),
        Value::Scalar(Scalar::Int(_)) => "int".into(),
        Value::Scalar(Scalar::Float(_)) => "float".into(),
        Value::Scalar(Scalar::String(_)) => "string".into(),
        Value::Scalar(Scalar::Bytes(_)) => "bytes".into(),
        Value::Hash(_) => "hash".into(),
        Value::Set(_) => "set".into(),
        Value::List(_) => "list".into(),
        Value::Tree(_) => "tree".into(),
    }
}

fn all_entries<S: Storage>(
    db: &Database<S>,
) -> Result<Vec<(DbPath, EntryKind, Option<Value>)>, alefs_namespace::NsError> {
    let mut out = Vec::new();
    walk(db, &DbPath::root(), &mut out)?;
    Ok(out)
}

fn walk<S: Storage>(
    db: &Database<S>,
    path: &DbPath,
    out: &mut Vec<(DbPath, EntryKind, Option<Value>)>,
) -> Result<(), alefs_namespace::NsError> {
    let entry = db.get(path)?;
    out.push((path.clone(), entry.kind, entry.value.clone()));
    if entry.kind == EntryKind::Directory {
        for (name, _) in db.list(path)? {
            let child = path.join(&name)?;
            walk(db, &child, out)?;
        }
    }
    Ok(())
}

fn eval_expr(expr: &Expr, path: &DbPath, kind: EntryKind, value: Option<&Value>) -> bool {
    match expr {
        Expr::And(a, b) => eval_expr(a, path, kind, value) && eval_expr(b, path, kind, value),
        Expr::Or(a, b) => eval_expr(a, path, kind, value) || eval_expr(b, path, kind, value),
        Expr::Not(a) => !eval_expr(a, path, kind, value),
        Expr::Pred(p) => eval_pred(p, path, kind, value),
    }
}

fn eval_pred(p: &Predicate, path: &DbPath, kind: EntryKind, value: Option<&Value>) -> bool {
    match p {
        Predicate::PathGlob(g) => path_glob_match(g, &path.as_str()),
        Predicate::NameGlob(g) => {
            let name = path.segments().last().map(|s| s.as_str()).unwrap_or("");
            glob_match(g, name)
        }
        Predicate::TypeName(t) => {
            let label = match (&kind, value) {
                (EntryKind::Directory, _) => "directory",
                (EntryKind::Value, Some(v)) => return type_matches(t, v),
                (EntryKind::Value, None) => "value",
            };
            // scalar umbrella
            if t == "scalar" {
                return matches!(value, Some(Value::Scalar(_)));
            }
            label == t.as_str()
        }
        Predicate::ValueCmp(op, s) => match value {
            Some(Value::Scalar(sv)) => cmp_scalar(op, sv, s),
            _ => false,
        },
        Predicate::HasKey(k) => match value {
            Some(Value::Hash(m)) => m.contains_key(k),
            Some(Value::Tree(m)) => {
                // try string key as Scalar::String
                m.contains_key(&Scalar::String(k.clone()))
                    || k.parse::<i64>()
                        .ok()
                        .map(|n| m.contains_key(&Scalar::Int(n)))
                        .unwrap_or(false)
            }
            _ => false,
        },
        Predicate::Contains(needle) => match value {
            Some(Value::Set(members)) => members.iter().any(|m| m == needle),
            Some(Value::List(items)) => items.iter().any(|m| m == needle),
            Some(Value::Hash(m)) => m.values().any(|m| m == needle),
            Some(Value::Tree(m)) => m.values().any(|m| m == needle),
            _ => false,
        },
        Predicate::AtIndex(idx, op, s) => match value {
            Some(Value::List(items)) => {
                if let Some(Value::Scalar(sv)) = items.get(*idx as usize) {
                    cmp_scalar(op, sv, s)
                } else {
                    false
                }
            }
            _ => false,
        },
        Predicate::KeyCmp(op, s) => match value {
            Some(Value::Tree(m)) => m.keys().any(|k| cmp_scalar(op, k, s)),
            _ => false,
        },
        Predicate::SizeCmp(op, n) => {
            let size = match (&kind, value) {
                (EntryKind::Directory, _) => return false, // could count children; skip
                (_, Some(Value::Hash(m))) => m.len() as i64,
                (_, Some(Value::Set(m))) => m.len() as i64,
                (_, Some(Value::List(m))) => m.len() as i64,
                (_, Some(Value::Tree(m))) => m.len() as i64,
                (_, Some(Value::Scalar(Scalar::String(s)))) => s.len() as i64,
                (_, Some(Value::Scalar(Scalar::Bytes(b)))) => b.len() as i64,
                _ => return false,
            };
            cmp_i64(op, size, *n)
        }
    }
}

fn type_matches(t: &str, v: &Value) -> bool {
    if t == "scalar" {
        return matches!(v, Value::Scalar(_));
    }
    type_label(v) == t
}

fn cmp_scalar(op: &CmpOp, a: &Scalar, b: &Scalar) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

fn cmp_i64(op: &CmpOp, a: i64, b: i64) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

fn path_glob_match(pattern: &str, path: &str) -> bool {
    if pattern == "/**" || pattern == "**" {
        return true;
    }
    // Support ** and *
    let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&parts, &path_parts)
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        // match any number of segments
        if pat.len() == 1 {
            return true;
        }
        for i in 0..=path.len() {
            if match_segments(&pat[1..], &path[i..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    if glob_match(pat[0], path[0]) {
        return match_segments(&pat[1..], &path[1..]);
    }
    false
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_rec(&p, &t)
}

fn glob_rec(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            for i in 0..=t.len() {
                if glob_rec(&p[1..], &t[i..]) {
                    return true;
                }
            }
            false
        }
        '?' => !t.is_empty() && glob_rec(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_rec(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alefs_namespace::Database;
    use alefs_storage::MemoryStorage;
    use alefs_types::Value;
    use std::collections::BTreeMap;

    fn setup() -> Database<MemoryStorage> {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        db.mkdir(&DbPath::parse("/users").unwrap()).unwrap();
        let mut map = BTreeMap::new();
        map.insert("email".into(), Value::string("a@b.c"));
        db.set(&DbPath::parse("/users/alice").unwrap(), Value::Hash(map))
            .unwrap();
        db.set(&DbPath::parse("/n").unwrap(), Value::int(3))
            .unwrap();
        db
    }

    #[test]
    fn query_hash_has() {
        let db = setup();
        let hits = execute(&db, r#"path /users/** AND type hash AND has "email""#).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path.as_str(), "/users/alice");
    }

    #[test]
    fn query_value() {
        let db = setup();
        let hits = execute(&db, "type int AND value = 3").unwrap();
        assert_eq!(hits.len(), 1);
    }
}
