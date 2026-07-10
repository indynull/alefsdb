use crate::error::NsError;
use crate::keys::{
    child_key, child_prefix, decode_id, encode_dir_node, encode_id, encode_value_node, meta_next_id,
    node_key, parse_node, type_index_key, type_index_prefix, NodeRecord, ROOT_ID,
};
use alefs_storage::{Storage, WriteBatch};
use alefs_types::{decode, encode, encode_payload, DbPath, Scalar, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: DbPath,
    pub kind: EntryKind,
    pub value: Option<Value>,
}


fn value_type_name(v: &Value) -> &'static str {
    v.typename()
}

fn node_type_name(rec: &NodeRecord) -> Result<&'static str, NsError> {
    match rec {
        NodeRecord::Dir => Ok("directory"),
        NodeRecord::Value(enc) => {
            let v = decode(enc)?;
            Ok(value_type_name(&v))
        }
    }
}

/// Database over any [`Storage`].
pub struct Database<S: Storage> {
    store: S,
}

impl<S: Storage> Database<S> {
    pub fn open(mut store: S) -> Result<Self, NsError> {
        // Ensure root exists.
        if store.get(&node_key(ROOT_ID))?.is_none() {
            let mut batch = WriteBatch::new();
            batch.put(node_key(ROOT_ID), encode_dir_node());
            batch.put(meta_next_id(), encode_id(ROOT_ID + 1));
            batch.put(type_index_key("directory", ROOT_ID), b"/".to_vec());
            store.commit(batch)?;
        } else if store.get(&meta_next_id())?.is_none() {
            // Recover next id if missing (shouldn't happen).
            let mut batch = WriteBatch::new();
            batch.put(meta_next_id(), encode_id(ROOT_ID + 1));
            store.commit(batch)?;
        }
        let mut db = Self { store };
        // Rebuild type index when missing/outdated (older data dirs).
        const INDEX_VER: u64 = 1;
        let ver_key = b"meta/type_index_version".as_slice();
        let needs = match db.store.get(ver_key)? {
            Some(v) if v.as_slice() == INDEX_VER.to_be_bytes() => false,
            _ => true,
        };
        if needs {
            db.rebuild_type_index()?;
            let mut batch = WriteBatch::new();
            batch.put(ver_key, INDEX_VER.to_be_bytes().to_vec());
            db.store.commit(batch)?;
        }
        Ok(db)
    }

    /// Walk the namespace and rewrite `idx/t/*` secondary indexes.
    pub fn rebuild_type_index(&mut self) -> Result<(), NsError> {
        // Delete existing type index keys.
        let mut batch = WriteBatch::new();
        for (k, _) in self.store.scan_prefix(b"idx/t/")? {
            batch.delete(k);
        }
        // Walk and re-insert.
        let mut stack = vec![DbPath::root()];
        while let Some(path) = stack.pop() {
            let (id, rec) = self.resolve(&path)?;
            let ty = node_type_name(&rec)?;
            batch.put(type_index_key(ty, id), path.as_str().into_bytes());
            if matches!(rec, NodeRecord::Dir) {
                for (name, _) in self.list(&path)? {
                    stack.push(path.join(&name)?);
                }
            }
        }
        self.store.commit(batch)?;
        Ok(())
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    pub fn into_store(self) -> S {
        self.store
    }

    fn next_id(&mut self) -> Result<u64, NsError> {
        let raw = self
            .store
            .get(&meta_next_id())?
            .ok_or_else(|| NsError::Invalid("missing next_id".into()))?;
        let id = decode_id(&raw).ok_or_else(|| NsError::Invalid("bad next_id".into()))?;
        Ok(id)
    }

    fn alloc_id(&mut self, batch: &mut WriteBatch) -> Result<u64, NsError> {
        let id = self.next_id()?;
        batch.put(meta_next_id(), encode_id(id + 1));
        Ok(id)
    }

    fn resolve(&self, path: &DbPath) -> Result<(u64, NodeRecord), NsError> {
        if path.is_root() {
            let bytes = self
                .store
                .get(&node_key(ROOT_ID))?
                .ok_or_else(|| NsError::NotFound("/".into()))?;
            let rec = parse_node(&bytes).map_err(NsError::Invalid)?;
            return Ok((ROOT_ID, rec));
        }
        let mut id = ROOT_ID;
        for seg in path.segments() {
            let child = self.store.get(&child_key(id, seg))?;
            let Some(raw) = child else {
                return Err(NsError::NotFound(path.as_str()));
            };
            id = decode_id(&raw).ok_or_else(|| NsError::Invalid("bad child id".into()))?;
        }
        let bytes = self
            .store
            .get(&node_key(id))?
            .ok_or_else(|| NsError::NotFound(path.as_str()))?;
        let rec = parse_node(&bytes).map_err(NsError::Invalid)?;
        Ok((id, rec))
    }

    pub fn exists(&self, path: &DbPath) -> Result<bool, NsError> {
        match self.resolve(path) {
            Ok(_) => Ok(true),
            Err(NsError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn get(&self, path: &DbPath) -> Result<Entry, NsError> {
        let (_id, rec) = self.resolve(path)?;
        match rec {
            NodeRecord::Dir => Ok(Entry {
                path: path.clone(),
                kind: EntryKind::Directory,
                value: None,
            }),
            NodeRecord::Value(enc) => {
                let value = decode(&enc)?;
                Ok(Entry {
                    path: path.clone(),
                    kind: EntryKind::Value,
                    value: Some(value),
                })
            }
        }
    }

    pub fn mkdir(&mut self, path: &DbPath) -> Result<(), NsError> {
        if path.is_root() {
            return Ok(());
        }
        if self.exists(path)? {
            return Err(NsError::AlreadyExists(path.as_str()));
        }
        let parent = path
            .parent()
            .ok_or_else(|| NsError::Invalid("no parent".into()))?;
        let name = path
            .segments()
            .last()
            .ok_or_else(|| NsError::Invalid("no name".into()))?;
        let (parent_id, parent_rec) = self.resolve(&parent).map_err(|e| match e {
            NsError::NotFound(_) => NsError::ParentMissing(parent.as_str()),
            other => other,
        })?;
        if !matches!(parent_rec, NodeRecord::Dir) {
            return Err(NsError::NotDirectory(parent.as_str()));
        }

        let mut batch = WriteBatch::new();
        let id = self.alloc_id(&mut batch)?;
        batch.put(node_key(id), encode_dir_node());
        batch.put(child_key(parent_id, name), encode_id(id));
        batch.put(type_index_key("directory", id), path.as_str().into_bytes());
        self.store.commit(batch)?;
        Ok(())
    }

    /// Create intermediate directories if missing? No — explicit mkdir only.
    /// Set a value at path (parent must exist as directory). Replaces existing value.
    pub fn set(&mut self, path: &DbPath, value: Value) -> Result<(), NsError> {
        if path.is_root() {
            return Err(NsError::Invalid("cannot set value at root".into()));
        }
        let parent = path
            .parent()
            .ok_or_else(|| NsError::Invalid("no parent".into()))?;
        let name = path
            .segments()
            .last()
            .ok_or_else(|| NsError::Invalid("no name".into()))?;

        let (parent_id, parent_rec) = self.resolve(&parent).map_err(|e| match e {
            NsError::NotFound(_) => NsError::ParentMissing(parent.as_str()),
            other => other,
        })?;
        if !matches!(parent_rec, NodeRecord::Dir) {
            return Err(NsError::NotDirectory(parent.as_str()));
        }

        let enc = encode(&value);
        let mut batch = WriteBatch::new();

        if let Some(raw) = self.store.get(&child_key(parent_id, name))? {
            let id = decode_id(&raw).ok_or_else(|| NsError::Invalid("bad child id".into()))?;
            let existing = self
                .store
                .get(&node_key(id))?
                .ok_or_else(|| NsError::Invalid("missing node".into()))?;
            match parse_node(&existing).map_err(NsError::Invalid)? {
                NodeRecord::Dir => return Err(NsError::IsDirectory(path.as_str())),
                NodeRecord::Value(old_enc) => {
                    let old_ty = decode(&old_enc)?.typename();
                    let new_ty = value.typename();
                    if old_ty != new_ty {
                        batch.delete(type_index_key(old_ty, id));
                    }
                    batch.put(node_key(id), encode_value_node(&enc));
                    batch.put(type_index_key(new_ty, id), path.as_str().into_bytes());
                }
            }
        } else {
            let id = self.alloc_id(&mut batch)?;
            batch.put(node_key(id), encode_value_node(&enc));
            batch.put(child_key(parent_id, name), encode_id(id));
            batch.put(type_index_key(value.typename(), id), path.as_str().into_bytes());
        }
        self.store.commit(batch)?;
        Ok(())
    }

    pub fn delete(&mut self, path: &DbPath) -> Result<(), NsError> {
        if path.is_root() {
            return Err(NsError::Invalid("cannot delete root".into()));
        }
        let (id, rec) = self.resolve(path)?;
        if matches!(rec, NodeRecord::Dir) {
            let children = self.store.scan_prefix(&child_prefix(id))?;
            if !children.is_empty() {
                return Err(NsError::Invalid(format!(
                    "directory not empty: {}",
                    path.as_str()
                )));
            }
        }
        let parent = path.parent().unwrap();
        let name = path.segments().last().unwrap();
        let (parent_id, _) = self.resolve(&parent)?;

        let mut batch = WriteBatch::new();
        let ty = node_type_name(&rec)?;
        batch.delete(type_index_key(ty, id));
        batch.delete(child_key(parent_id, name));
        batch.delete(node_key(id));
        self.store.commit(batch)?;
        Ok(())
    }

    /// Paths of all value/dir nodes with the given type name (secondary index).
    pub fn paths_with_type(&self, type_name: &str) -> Result<Vec<DbPath>, NsError> {
        let rows = self.store.scan_prefix(&type_index_prefix(type_name))?;
        let mut out = Vec::new();
        for (_k, v) in rows {
            let s = String::from_utf8(v).map_err(|_| NsError::Invalid("idx path utf8".into()))?;
            out.push(DbPath::parse(&s)?);
        }
        out.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        Ok(out)
    }

    /// List directory children as (name, kind).
    pub fn list(&self, path: &DbPath) -> Result<Vec<(String, EntryKind)>, NsError> {
        let (id, rec) = self.resolve(path)?;
        if !matches!(rec, NodeRecord::Dir) {
            return Err(NsError::NotDirectory(path.as_str()));
        }
        let prefix = child_prefix(id);
        let rows = self.store.scan_prefix(&prefix)?;
        let mut out = Vec::new();
        for (k, v) in rows {
            let name = String::from_utf8(k[prefix.len()..].to_vec())
                .map_err(|_| NsError::Invalid("child name not utf8".into()))?;
            let child_id = decode_id(&v).ok_or_else(|| NsError::Invalid("bad child id".into()))?;
            let bytes = self
                .store
                .get(&node_key(child_id))?
                .ok_or_else(|| NsError::Invalid("missing child node".into()))?;
            let kind = match parse_node(&bytes).map_err(NsError::Invalid)? {
                NodeRecord::Dir => EntryKind::Directory,
                NodeRecord::Value(_) => EntryKind::Value,
            };
            out.push((name, kind));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Walk all value entries (and optionally dirs) depth-first.
    pub fn walk_values(&self) -> Result<Vec<(DbPath, Value)>, NsError> {
        let mut out = Vec::new();
        self.walk_rec(&DbPath::root(), &mut out)?;
        Ok(out)
    }

    fn walk_rec(&self, path: &DbPath, out: &mut Vec<(DbPath, Value)>) -> Result<(), NsError> {
        let entry = self.get(path)?;
        match entry.kind {
            EntryKind::Directory => {
                for (name, _) in self.list(path)? {
                    let child = path.join(&name)?;
                    self.walk_rec(&child, out)?;
                }
            }
            EntryKind::Value => {
                if let Some(v) = entry.value {
                    out.push((path.clone(), v));
                }
            }
        }
        Ok(())
    }

    /// Export as nested JSON-like tree for tooling (serde-free simple format).
    pub fn export_json(&self) -> Result<String, NsError> {
        let tree = self.export_node(&DbPath::root())?;
        Ok(tree)
    }

    fn export_node(&self, path: &DbPath) -> Result<String, NsError> {
        let entry = self.get(path)?;
        match entry.kind {
            EntryKind::Directory => {
                let mut parts = Vec::new();
                for (name, _) in self.list(path)? {
                    let child = path.join(&name)?;
                    let body = self.export_node(&child)?;
                    parts.push(format!("{}: {}", json_str(&name), body));
                }
                // Directories use an explicit wrapper so hashes stay typed values.
                Ok(format!("{{\"__dir\": {{{}}}}}", parts.join(", ")))
            }
            EntryKind::Value => Ok(value_to_json(&entry.value.unwrap())),
        }
    }

    pub fn import_json(&mut self, json: &str) -> Result<(), NsError> {
        let v = parse_json_value(json.trim()).map_err(NsError::Invalid)?;
        self.import_node(&DbPath::root(), v, true)?;
        Ok(())
    }

    fn import_node(&mut self, path: &DbPath, v: JsonVal, at_root: bool) -> Result<(), NsError> {
        match v {
            JsonVal::Object(map) if map.len() == 1 && map[0].0 == "__dir" => {
                let JsonVal::Object(children) = map.into_iter().next().unwrap().1 else {
                    return Err(NsError::Invalid("__dir must be object".into()));
                };
                if !at_root && !path.is_root() && !self.exists(path)? {
                    self.mkdir(path)?;
                }
                for (k, child_v) in children {
                    let child_path = if path.is_root() {
                        DbPath::parse(&format!("/{k}"))?
                    } else {
                        path.join(&k)?
                    };
                    self.import_node(&child_path, child_v, false)?;
                }
                Ok(())
            }
            other => {
                if at_root {
                    // Allow bare object at root as directory for convenience.
                    if let JsonVal::Object(map) = other {
                        for (k, child_v) in map {
                            let child_path = DbPath::parse(&format!("/{k}"))?;
                            self.import_node(&child_path, child_v, false)?;
                        }
                        return Ok(());
                    }
                    return Err(NsError::Invalid("root import must be object".into()));
                }
                let val = json_to_value(other)?;
                self.set(path, val)
            }
        }
    }

    // ----- structure helpers for hash/set/list/tree -----

    pub fn hash_set(&mut self, path: &DbPath, key: &str, value: Value) -> Result<(), NsError> {
        let mut map = match self.get(path) {
            Ok(e) => match e.value {
                Some(Value::Hash(m)) => m,
                Some(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a hash",
                        path.as_str()
                    )))
                }
                None => return Err(NsError::IsDirectory(path.as_str())),
            },
            Err(NsError::NotFound(_)) => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        map.insert(key.to_owned(), value);
        self.set(path, Value::Hash(map))
    }

    pub fn hash_get(&self, path: &DbPath, key: &str) -> Result<Option<Value>, NsError> {
        match self.get(path)?.value {
            Some(Value::Hash(m)) => Ok(m.get(key).cloned()),
            Some(_) => Err(NsError::TypeMismatch(format!(
                "{} is not a hash",
                path.as_str()
            ))),
            None => Err(NsError::IsDirectory(path.as_str())),
        }
    }

    pub fn list_push(&mut self, path: &DbPath, value: Value) -> Result<(), NsError> {
        let mut items = match self.get(path) {
            Ok(e) => match e.value {
                Some(Value::List(v)) => v,
                Some(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a list",
                        path.as_str()
                    )))
                }
                None => return Err(NsError::IsDirectory(path.as_str())),
            },
            Err(NsError::NotFound(_)) => Vec::new(),
            Err(e) => return Err(e),
        };
        items.push(value);
        self.set(path, Value::List(items))
    }

    pub fn set_add(&mut self, path: &DbPath, value: Value) -> Result<(), NsError> {
        let mut members = match self.get(path) {
            Ok(e) => match e.value {
                Some(Value::Set(v)) => v,
                Some(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a set",
                        path.as_str()
                    )))
                }
                None => return Err(NsError::IsDirectory(path.as_str())),
            },
            Err(NsError::NotFound(_)) => Vec::new(),
            Err(e) => return Err(e),
        };
        let payload = encode_payload(&value);
        if !members.iter().any(|m| encode_payload(m) == payload) {
            members.push(value);
        }
        self.set(path, Value::Set(members))
    }

    pub fn tree_set(&mut self, path: &DbPath, key: Scalar, value: Value) -> Result<(), NsError> {
        let mut map = match self.get(path) {
            Ok(e) => match e.value {
                Some(Value::Tree(m)) => m,
                Some(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a tree",
                        path.as_str()
                    )))
                }
                None => return Err(NsError::IsDirectory(path.as_str())),
            },
            Err(NsError::NotFound(_)) => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        map.insert(key, value);
        self.set(path, Value::Tree(map))
    }
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn value_to_json(v: &Value) -> String {
    match v {
        Value::Scalar(Scalar::Null) => "null".into(),
        Value::Scalar(Scalar::Bool(b)) => b.to_string(),
        Value::Scalar(Scalar::Int(n)) => n.to_string(),
        Value::Scalar(Scalar::Float(bits)) => {
            let f = f64::from_bits(*bits);
            if f.is_finite() {
                format!("{f}")
            } else {
                json_str(&format!("float:{bits}"))
            }
        }
        Value::Scalar(Scalar::String(s)) => json_str(s),
        Value::Scalar(Scalar::Bytes(b)) => {
            format!("{{\"__bytes\":{}}}", json_str(&hex::encode_bytes(b)))
        }
        Value::Hash(m) => {
            let parts: Vec<_> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", json_str(k), value_to_json(v)))
                .collect();
            format!("{{\"__hash\": {{{}}}}}", parts.join(", "))
        }
        Value::List(items) => {
            let parts: Vec<_> = items.iter().map(value_to_json).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Set(items) => {
            let parts: Vec<_> = items.iter().map(value_to_json).collect();
            format!("{{\"__set\": [{}]}}", parts.join(", "))
        }
        Value::Tree(m) => {
            let parts: Vec<_> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", json_str(&scalar_key_str(k)), value_to_json(v)))
                .collect();
            format!("{{\"__tree\": {{{}}}}}", parts.join(", "))
        }
    }
}

fn scalar_key_str(s: &Scalar) -> String {
    match s {
        Scalar::Null => "null".into(),
        Scalar::Bool(b) => b.to_string(),
        Scalar::Int(n) => n.to_string(),
        Scalar::Float(bits) => format!("f:{}", f64::from_bits(*bits)),
        Scalar::String(s) => s.clone(),
        Scalar::Bytes(b) => format!("b:{}", hex::encode_bytes(b)),
    }
}

mod hex {
    pub fn encode_bytes(b: &[u8]) -> String {
        const HEX: &[u8] = b"0123456789abcdef";
        let mut s = String::with_capacity(b.len() * 2);
        for &x in b {
            s.push(HEX[(x >> 4) as usize] as char);
            s.push(HEX[(x & 0xf) as usize] as char);
        }
        s
    }

    pub fn decode_bytes(s: &str) -> Result<Vec<u8>, String> {
        if !s.len().is_multiple_of(2) {
            return Err("odd hex length".into());
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let hi = from_hex(bytes[i])?;
            let lo = from_hex(bytes[i + 1])?;
            out.push((hi << 4) | lo);
            i += 2;
        }
        Ok(out)
    }

    fn from_hex(c: u8) -> Result<u8, String> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err("bad hex".into()),
        }
    }
}

#[derive(Debug)]
enum JsonVal {
    Null,
    Bool(bool),
    Number(i64),
    Float(f64),
    String(String),
    Array(Vec<JsonVal>),
    Object(Vec<(String, JsonVal)>),
}

fn parse_json_value(s: &str) -> Result<JsonVal, String> {
    let mut p = JsonParser { s, i: 0 };
    let v = p.parse_value()?;
    p.skip_ws();
    if p.i != p.s.len() {
        return Err("trailing junk in json".into());
    }
    Ok(v)
}

struct JsonParser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn parse_value(&mut self) -> Result<JsonVal, String> {
        self.skip_ws();
        match self.peek() {
            Some('n') => self.parse_null(),
            Some('t') | Some('f') => self.parse_bool(),
            Some('"') => Ok(JsonVal::String(self.parse_string()?)),
            Some('[') => self.parse_array(),
            Some('{') => self.parse_object(),
            Some('-') | Some('0'..='9') => self.parse_number(),
            Some(c) => Err(format!("unexpected {c}")),
            None => Err("unexpected eof".into()),
        }
    }

    fn parse_null(&mut self) -> Result<JsonVal, String> {
        if self.s[self.i..].starts_with("null") {
            self.i += 4;
            Ok(JsonVal::Null)
        } else {
            Err("expected null".into())
        }
    }

    fn parse_bool(&mut self) -> Result<JsonVal, String> {
        if self.s[self.i..].starts_with("true") {
            self.i += 4;
            Ok(JsonVal::Bool(true))
        } else if self.s[self.i..].starts_with("false") {
            self.i += 5;
            Ok(JsonVal::Bool(false))
        } else {
            Err("expected bool".into())
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.bump() != Some('"') {
            return Err("expected string".into());
        }
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => return Ok(out),
                '\\' => match self.bump() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(o) => out.push(o),
                    None => return Err("bad escape".into()),
                },
                other => out.push(other),
            }
        }
        Err("unterminated string".into())
    }

    fn parse_number(&mut self) -> Result<JsonVal, String> {
        let start = self.i;
        if self.peek() == Some('-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some('0'..='9')) {
            self.i += 1;
        }
        let mut is_float = false;
        if self.peek() == Some('.') {
            is_float = true;
            self.i += 1;
            while matches!(self.peek(), Some('0'..='9')) {
                self.i += 1;
            }
        }
        let text = &self.s[start..self.i];
        if is_float {
            Ok(JsonVal::Float(
                text.parse().map_err(|_| "bad float".to_string())?,
            ))
        } else {
            Ok(JsonVal::Number(
                text.parse().map_err(|_| "bad int".to_string())?,
            ))
        }
    }

    fn parse_array(&mut self) -> Result<JsonVal, String> {
        self.bump(); // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(JsonVal::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => break,
                _ => return Err("expected , or ]".into()),
            }
        }
        Ok(JsonVal::Array(items))
    }

    fn parse_object(&mut self) -> Result<JsonVal, String> {
        self.bump(); // {
        let mut map = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(JsonVal::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(':') {
                return Err("expected :".into());
            }
            let val = self.parse_value()?;
            map.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err("expected , or }".into()),
            }
        }
        Ok(JsonVal::Object(map))
    }
}

fn json_to_value(v: JsonVal) -> Result<Value, NsError> {
    match v {
        JsonVal::Null => Ok(Value::null()),
        JsonVal::Bool(b) => Ok(Value::bool(b)),
        JsonVal::Number(n) => Ok(Value::int(n)),
        JsonVal::Float(f) => Ok(Value::float(f)),
        JsonVal::String(s) => Ok(Value::string(s)),
        JsonVal::Array(items) => {
            let mut list = Vec::new();
            for i in items {
                list.push(json_to_value(i)?);
            }
            Ok(Value::List(list))
        }
        JsonVal::Object(mut map) => {
            // special forms
            if map.len() == 1 {
                let (k, v) = map.pop().unwrap();
                match (k.as_str(), v) {
                    ("__bytes", JsonVal::String(h)) => {
                        let b = hex::decode_bytes(&h).map_err(NsError::Invalid)?;
                        return Ok(Value::bytes(b));
                    }
                    ("__set", JsonVal::Array(items)) => {
                        let mut set = Vec::new();
                        for i in items {
                            set.push(json_to_value(i)?);
                        }
                        return Ok(Value::Set(set));
                    }
                    ("__hash", JsonVal::Object(inner)) => {
                        let mut hash = BTreeMap::new();
                        for (hk, hv) in inner {
                            hash.insert(hk, json_to_value(hv)?);
                        }
                        return Ok(Value::Hash(hash));
                    }
                    ("__tree", JsonVal::Object(inner)) => {
                        let mut tree = BTreeMap::new();
                        for (tk, tv) in inner {
                            let sk = parse_scalar_key(&tk)?;
                            tree.insert(sk, json_to_value(tv)?);
                        }
                        return Ok(Value::Tree(tree));
                    }
                    (other_k, other_v) => {
                        map.push((other_k.to_string(), other_v));
                    }
                }
            }
            let mut hash = BTreeMap::new();
            for (k, v) in map {
                hash.insert(k, json_to_value(v)?);
            }
            Ok(Value::Hash(hash))
        }
    }
}

fn parse_scalar_key(s: &str) -> Result<Scalar, NsError> {
    if s == "null" {
        return Ok(Scalar::Null);
    }
    if s == "true" {
        return Ok(Scalar::Bool(true));
    }
    if s == "false" {
        return Ok(Scalar::Bool(false));
    }
    if let Some(rest) = s.strip_prefix("f:") {
        let f: f64 = rest.parse().map_err(|e| NsError::Invalid(format!("{e}")))?;
        return Ok(Scalar::from_f64(f));
    }
    if let Some(rest) = s.strip_prefix("b:") {
        let b = hex::decode_bytes(rest).map_err(NsError::Invalid)?;
        return Ok(Scalar::Bytes(b));
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Scalar::Int(n));
    }
    Ok(Scalar::String(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alefs_storage::{MemoryStorage, WalStorage};

    #[test]
    fn mkdir_set_get_list_scalar() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        let users = DbPath::parse("/users").unwrap();
        db.mkdir(&users).unwrap();
        let alice = DbPath::parse("/users/alice").unwrap();
        db.set(&alice, Value::string("hi")).unwrap();
        let e = db.get(&alice).unwrap();
        assert_eq!(e.value, Some(Value::string("hi")));
        let kids = db.list(&users).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].0, "alice");
    }

    #[test]
    fn explicit_mkdir_required() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        let p = DbPath::parse("/nope/x").unwrap();
        let err = db.set(&p, Value::int(1)).unwrap_err();
        assert!(matches!(err, NsError::ParentMissing(_)));
    }

    #[test]
    fn durable_namespace() {
        let dir = std::env::temp_dir().join(format!("alefs-ns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let store = WalStorage::open(&dir).unwrap();
            let mut db = Database::open(store).unwrap();
            db.mkdir(&DbPath::parse("/a").unwrap()).unwrap();
            db.set(&DbPath::parse("/a/x").unwrap(), Value::int(7))
                .unwrap();
        }
        {
            let store = WalStorage::open(&dir).unwrap();
            let db = Database::open(store).unwrap();
            let e = db.get(&DbPath::parse("/a/x").unwrap()).unwrap();
            assert_eq!(e.value, Some(Value::int(7)));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_list_set_tree_ops() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        let h = DbPath::parse("/h").unwrap();
        db.hash_set(&h, "k", Value::int(1)).unwrap();
        assert_eq!(db.hash_get(&h, "k").unwrap(), Some(Value::int(1)));

        let l = DbPath::parse("/l").unwrap();
        db.list_push(&l, Value::string("a")).unwrap();
        db.list_push(&l, Value::string("b")).unwrap();
        match db.get(&l).unwrap().value.unwrap() {
            Value::List(v) => assert_eq!(v.len(), 2),
            _ => panic!(),
        }

        let s = DbPath::parse("/s").unwrap();
        db.set_add(&s, Value::int(1)).unwrap();
        db.set_add(&s, Value::int(1)).unwrap();
        db.set_add(&s, Value::int(2)).unwrap();
        match db.get(&s).unwrap().value.unwrap() {
            Value::Set(v) => assert_eq!(v.len(), 2),
            _ => panic!(),
        }

        let t = DbPath::parse("/t").unwrap();
        db.tree_set(&t, Scalar::Int(10), Value::bool(true)).unwrap();
        match db.get(&t).unwrap().value.unwrap() {
            Value::Tree(m) => assert!(m.contains_key(&Scalar::Int(10))),
            _ => panic!(),
        }
    }

    #[test]
    fn export_import_roundtrip() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        db.mkdir(&DbPath::parse("/d").unwrap()).unwrap();
        db.set(&DbPath::parse("/d/x").unwrap(), Value::string("z"))
            .unwrap();
        let json = db.export_json().unwrap();
        let mut db2 = Database::open(MemoryStorage::new()).unwrap();
        db2.import_json(&json).unwrap();
        assert_eq!(
            db2.get(&DbPath::parse("/d/x").unwrap())
                .unwrap()
                .value
                .unwrap(),
            Value::string("z")
        );
    }
}

    #[cfg(test)]
    mod index_tests {
        use super::*;
        use alefs_storage::MemoryStorage;

        #[test]
        fn type_index_lists_paths() {
            let mut db = Database::open(MemoryStorage::new()).unwrap();
            db.mkdir(&DbPath::parse("/a").unwrap()).unwrap();
            db.set(&DbPath::parse("/a/x").unwrap(), Value::int(1))
                .unwrap();
            db.set(&DbPath::parse("/a/y").unwrap(), Value::string("s"))
                .unwrap();
            let ints = db.paths_with_type("int").unwrap();
            assert_eq!(ints.len(), 1);
            assert_eq!(ints[0].as_str(), "/a/x");
            let strings = db.paths_with_type("string").unwrap();
            assert!(strings.iter().any(|p| p.as_str() == "/a/y"));
        }
    }
