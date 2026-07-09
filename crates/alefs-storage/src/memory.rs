use crate::batch::{BatchOp, WriteBatch};
use crate::error::StorageError;
use crate::Storage;
use std::collections::BTreeMap;

/// In-memory storage (design S0). Not the default durable `serve` backend.
#[derive(Debug, Default, Clone)]
pub struct MemoryStorage {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Storage for MemoryStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.map.get(key).cloned())
    }

    fn commit(&mut self, batch: WriteBatch) -> Result<(), StorageError> {
        for op in batch.ops {
            match op {
                BatchOp::Put { key, value } => {
                    self.map.insert(key, value);
                }
                BatchOp::Delete { key } => {
                    self.map.remove(&key);
                }
            }
        }
        Ok(())
    }

    fn scan_prefix(&self, prefix: &[u8]) -> Result<crate::ScanRows, StorageError> {
        let mut out = Vec::new();
        for (k, v) in self.map.range(prefix.to_vec()..) {
            if !k.starts_with(prefix) {
                break;
            }
            out.push((k.clone(), v.clone()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_delete() {
        let mut s = MemoryStorage::new();
        let mut batch = WriteBatch::new();
        batch.put(b"a", b"1");
        batch.put(b"b", b"2");
        s.commit(batch).unwrap();
        assert_eq!(s.get(b"a").unwrap().as_deref(), Some(b"1".as_ref()));
        assert_eq!(s.get(b"missing").unwrap(), None);

        let mut batch = WriteBatch::new();
        batch.delete(b"a");
        s.commit(batch).unwrap();
        assert_eq!(s.get(b"a").unwrap(), None);
        assert_eq!(s.get(b"b").unwrap().as_deref(), Some(b"2".as_ref()));
    }

    #[test]
    fn scan_prefix_ordered() {
        let mut s = MemoryStorage::new();
        let mut batch = WriteBatch::new();
        batch.put(b"user/b", b"2");
        batch.put(b"user/a", b"1");
        batch.put(b"other", b"x");
        s.commit(batch).unwrap();

        let rows = s.scan_prefix(b"user/").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, b"user/a");
        assert_eq!(rows[1].0, b"user/b");
    }

    #[test]
    fn empty_batch_ok() {
        let mut s = MemoryStorage::new();
        s.commit(WriteBatch::new()).unwrap();
        assert!(s.is_empty());
    }
}
