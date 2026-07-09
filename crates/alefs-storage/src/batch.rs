/// A single mutation in a write batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchOp {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

/// Atomic group of puts/deletes applied on [`crate::Storage::commit`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteBatch {
    pub(crate) ops: Vec<BatchOp>,
}

impl WriteBatch {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.ops.push(BatchOp::Put {
            key: key.into(),
            value: value.into(),
        });
    }

    pub fn delete(&mut self, key: impl Into<Vec<u8>>) {
        self.ops.push(BatchOp::Delete { key: key.into() });
    }

    pub fn ops(&self) -> &[BatchOp] {
        &self.ops
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }
}
