//! Atomic multi-op transactions via a write overlay + single commit.

use crate::db::Database;
use crate::error::NsError;
use crate::keys::{
    child_key, child_prefix, decode_id, encode_dir_node, encode_id, encode_value_node,
    meta_next_id, node_key, parse_node, NodeRecord, ROOT_ID,
};
use alefs_storage::{Storage, WriteBatch};
use alefs_types::{decode, encode, encode_payload, DbPath, Scalar, Value};
use std::collections::BTreeMap;

/// One mutation inside a transaction.
#[derive(Debug, Clone)]
pub enum TxnOp {
    Mkdir { path: DbPath },
    Set { path: DbPath, value: Value },
    Delete { path: DbPath },
    HashSet {
        path: DbPath,
        key: String,
        value: Value,
    },
    ListPush { path: DbPath, value: Value },
    SetAdd { path: DbPath, value: Value },
    TreeSet {
        path: DbPath,
        key: Scalar,
        value: Value,
    },
}

/// Overlay over Storage for transactional reads/writes before one commit.
struct Overlay<'a, S: Storage> {
    store: &'a S,
    /// None value means deleted.
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl<'a, S: Storage> Overlay<'a, S> {
    fn new(store: &'a S) -> Self {
        Self {
            store,
            pending: BTreeMap::new(),
        }
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, NsError> {
        if let Some(v) = self.pending.get(key) {
            return Ok(v.clone());
        }
        Ok(self.store.get(key)?)
    }

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.pending.insert(key, Some(value));
    }

    fn delete(&mut self, key: Vec<u8>) {
        self.pending.insert(key, None);
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, NsError> {
        let mut map: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for (k, v) in self.store.scan_prefix(prefix)? {
            map.insert(k, v);
        }
        for (k, v) in &self.pending {
            if !k.starts_with(prefix) {
                continue;
            }
            match v {
                Some(val) => {
                    map.insert(k.clone(), val.clone());
                }
                None => {
                    map.remove(k);
                }
            }
        }
        Ok(map.into_iter().collect())
    }

    fn into_batch(self) -> WriteBatch {
        let mut batch = WriteBatch::new();
        for (k, v) in self.pending {
            match v {
                Some(val) => batch.put(k, val),
                None => batch.delete(k),
            }
        }
        batch
    }
}

impl<S: Storage> Database<S> {
    /// Apply all ops atomically (single storage commit). Empty ops are a no-op.
    pub fn apply_txn(&mut self, ops: &[TxnOp]) -> Result<(), NsError> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut ov = Overlay::new(self.store());
        for op in ops {
            apply_one(&mut ov, op)?;
        }
        let batch = ov.into_batch();
        if !batch.is_empty() {
            self.store_mut().commit(batch)?;
        }
        Ok(())
    }
}

fn apply_one<S: Storage>(ov: &mut Overlay<'_, S>, op: &TxnOp) -> Result<(), NsError> {
    match op {
        TxnOp::Mkdir { path } => mkdir_ov(ov, path),
        TxnOp::Set { path, value } => set_ov(ov, path, value.clone()),
        TxnOp::Delete { path } => delete_ov(ov, path),
        TxnOp::HashSet { path, key, value } => {
            let mut map = match get_value_ov(ov, path) {
                Ok(Value::Hash(m)) => m,
                Ok(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a hash",
                        path.as_str()
                    )))
                }
                Err(NsError::NotFound(_)) => BTreeMap::new(),
                Err(e) => return Err(e),
            };
            map.insert(key.clone(), value.clone());
            set_ov(ov, path, Value::Hash(map))
        }
        TxnOp::ListPush { path, value } => {
            let mut items = match get_value_ov(ov, path) {
                Ok(Value::List(v)) => v,
                Ok(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a list",
                        path.as_str()
                    )))
                }
                Err(NsError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            };
            items.push(value.clone());
            set_ov(ov, path, Value::List(items))
        }
        TxnOp::SetAdd { path, value } => {
            let mut members = match get_value_ov(ov, path) {
                Ok(Value::Set(v)) => v,
                Ok(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a set",
                        path.as_str()
                    )))
                }
                Err(NsError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            };
            let payload = encode_payload(value);
            if !members.iter().any(|m| encode_payload(m) == payload) {
                members.push(value.clone());
            }
            set_ov(ov, path, Value::Set(members))
        }
        TxnOp::TreeSet { path, key, value } => {
            let mut map = match get_value_ov(ov, path) {
                Ok(Value::Tree(m)) => m,
                Ok(_) => {
                    return Err(NsError::TypeMismatch(format!(
                        "{} is not a tree",
                        path.as_str()
                    )))
                }
                Err(NsError::NotFound(_)) => BTreeMap::new(),
                Err(e) => return Err(e),
            };
            map.insert(key.clone(), value.clone());
            set_ov(ov, path, Value::Tree(map))
        }
    }
}

fn next_id_ov<S: Storage>(ov: &Overlay<'_, S>) -> Result<u64, NsError> {
    let raw = ov
        .get(&meta_next_id())?
        .ok_or_else(|| NsError::Invalid("missing next_id".into()))?;
    decode_id(&raw).ok_or_else(|| NsError::Invalid("bad next_id".into()))
}

fn resolve_ov<S: Storage>(
    ov: &Overlay<'_, S>,
    path: &DbPath,
) -> Result<(u64, NodeRecord), NsError> {
    if path.is_root() {
        let bytes = ov
            .get(&node_key(ROOT_ID))?
            .ok_or_else(|| NsError::NotFound("/".into()))?;
        let rec = parse_node(&bytes).map_err(NsError::Invalid)?;
        return Ok((ROOT_ID, rec));
    }
    let mut id = ROOT_ID;
    for seg in path.segments() {
        let child = ov.get(&child_key(id, seg))?;
        let Some(raw) = child else {
            return Err(NsError::NotFound(path.as_str()));
        };
        id = decode_id(&raw).ok_or_else(|| NsError::Invalid("bad child id".into()))?;
    }
    let bytes = ov
        .get(&node_key(id))?
        .ok_or_else(|| NsError::NotFound(path.as_str()))?;
    let rec = parse_node(&bytes).map_err(NsError::Invalid)?;
    Ok((id, rec))
}

fn get_value_ov<S: Storage>(ov: &Overlay<'_, S>, path: &DbPath) -> Result<Value, NsError> {
    let (_id, rec) = resolve_ov(ov, path)?;
    match rec {
        NodeRecord::Dir => Err(NsError::IsDirectory(path.as_str())),
        NodeRecord::Value(enc) => Ok(decode(&enc)?),
    }
}

fn mkdir_ov<S: Storage>(ov: &mut Overlay<'_, S>, path: &DbPath) -> Result<(), NsError> {
    if path.is_root() {
        return Ok(());
    }
    if resolve_ov(ov, path).is_ok() {
        return Err(NsError::AlreadyExists(path.as_str()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| NsError::Invalid("no parent".into()))?;
    let name = path
        .segments()
        .last()
        .ok_or_else(|| NsError::Invalid("no name".into()))?;
    let (parent_id, parent_rec) = resolve_ov(ov, &parent).map_err(|e| match e {
        NsError::NotFound(_) => NsError::ParentMissing(parent.as_str()),
        other => other,
    })?;
    if !matches!(parent_rec, NodeRecord::Dir) {
        return Err(NsError::NotDirectory(parent.as_str()));
    }
    let id = next_id_ov(ov)?;
    ov.put(meta_next_id(), encode_id(id + 1));
    ov.put(node_key(id), encode_dir_node());
    ov.put(child_key(parent_id, name), encode_id(id));
    Ok(())
}

fn set_ov<S: Storage>(
    ov: &mut Overlay<'_, S>,
    path: &DbPath,
    value: Value,
) -> Result<(), NsError> {
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
    let (parent_id, parent_rec) = resolve_ov(ov, &parent).map_err(|e| match e {
        NsError::NotFound(_) => NsError::ParentMissing(parent.as_str()),
        other => other,
    })?;
    if !matches!(parent_rec, NodeRecord::Dir) {
        return Err(NsError::NotDirectory(parent.as_str()));
    }
    let enc = encode(&value);
    if let Some(raw) = ov.get(&child_key(parent_id, name))? {
        let id = decode_id(&raw).ok_or_else(|| NsError::Invalid("bad child id".into()))?;
        let existing = ov
            .get(&node_key(id))?
            .ok_or_else(|| NsError::Invalid("missing node".into()))?;
        match parse_node(&existing).map_err(NsError::Invalid)? {
            NodeRecord::Dir => return Err(NsError::IsDirectory(path.as_str())),
            NodeRecord::Value(_) => {
                ov.put(node_key(id), encode_value_node(&enc));
            }
        }
    } else {
        let id = next_id_ov(ov)?;
        ov.put(meta_next_id(), encode_id(id + 1));
        ov.put(node_key(id), encode_value_node(&enc));
        ov.put(child_key(parent_id, name), encode_id(id));
    }
    Ok(())
}

fn delete_ov<S: Storage>(ov: &mut Overlay<'_, S>, path: &DbPath) -> Result<(), NsError> {
    if path.is_root() {
        return Err(NsError::Invalid("cannot delete root".into()));
    }
    let (id, rec) = resolve_ov(ov, path)?;
    if matches!(rec, NodeRecord::Dir) {
        let children = ov.scan_prefix(&child_prefix(id))?;
        if !children.is_empty() {
            return Err(NsError::Invalid(format!(
                "directory not empty: {}",
                path.as_str()
            )));
        }
    }
    let parent = path.parent().unwrap();
    let name = path.segments().last().unwrap();
    let (parent_id, _) = resolve_ov(ov, &parent)?;
    ov.delete(child_key(parent_id, name));
    ov.delete(node_key(id));
    Ok(())
}

// silence unused import warnings for Entry if not used
#[allow(dead_code)]
fn _entry_kind(_: EntryKind, _: Entry) {}

#[cfg(test)]
mod tests {
    use super::*;
    use alefs_storage::{MemoryStorage, WalStorage};
    use alefs_types::DbPath;

    #[test]
    fn txn_atomic_mkdir_and_set() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        db.apply_txn(&[
            TxnOp::Mkdir {
                path: DbPath::parse("/a").unwrap(),
            },
            TxnOp::Set {
                path: DbPath::parse("/a/x").unwrap(),
                value: Value::int(1),
            },
        ])
        .unwrap();
        assert_eq!(
            db.get(&DbPath::parse("/a/x").unwrap())
                .unwrap()
                .value
                .unwrap(),
            Value::int(1)
        );
    }

    #[test]
    fn txn_rolls_back_on_error() {
        let mut db = Database::open(MemoryStorage::new()).unwrap();
        let err = db
            .apply_txn(&[
                TxnOp::Mkdir {
                    path: DbPath::parse("/a").unwrap(),
                },
                TxnOp::Set {
                    path: DbPath::parse("/missing/x").unwrap(),
                    value: Value::int(1),
                },
            ])
            .unwrap_err();
        assert!(matches!(err, NsError::ParentMissing(_)));
        // nothing from the failed txn committed
        assert!(!db.exists(&DbPath::parse("/a").unwrap()).unwrap());
    }

    #[test]
    fn txn_durable_single_commit() {
        let dir = std::env::temp_dir().join(format!(
            "alefs-txn-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let store = WalStorage::open(&dir).unwrap();
            let mut db = Database::open(store).unwrap();
            db.apply_txn(&[
                TxnOp::Mkdir {
                    path: DbPath::parse("/t").unwrap(),
                },
                TxnOp::Set {
                    path: DbPath::parse("/t/v").unwrap(),
                    value: Value::string("ok"),
                },
            ])
            .unwrap();
        }
        {
            let store = WalStorage::open(&dir).unwrap();
            let db = Database::open(store).unwrap();
            assert_eq!(
                db.get(&DbPath::parse("/t/v").unwrap())
                    .unwrap()
                    .value
                    .unwrap(),
                Value::string("ok")
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
