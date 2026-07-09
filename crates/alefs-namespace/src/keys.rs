//! Storage key layout.

pub const ROOT_ID: u64 = 1;

pub fn meta_next_id() -> Vec<u8> {
    b"meta/next_id".to_vec()
}

pub fn node_key(id: u64) -> Vec<u8> {
    let mut k = b"node/".to_vec();
    k.extend_from_slice(&id.to_be_bytes());
    k
}

pub fn child_key(parent: u64, name: &str) -> Vec<u8> {
    let mut k = b"child/".to_vec();
    k.extend_from_slice(&parent.to_be_bytes());
    k.push(b'/');
    k.extend_from_slice(name.as_bytes());
    k
}

pub fn child_prefix(parent: u64) -> Vec<u8> {
    let mut k = b"child/".to_vec();
    k.extend_from_slice(&parent.to_be_bytes());
    k.push(b'/');
    k
}

pub fn encode_id(id: u64) -> Vec<u8> {
    id.to_be_bytes().to_vec()
}

pub fn decode_id(bytes: &[u8]) -> Option<u64> {
    if bytes.len() != 8 {
        return None;
    }
    let mut a = [0u8; 8];
    a.copy_from_slice(bytes);
    Some(u64::from_be_bytes(a))
}

/// Node payload: tag 0 = dir, tag 1 = value + encoded Value.
pub fn encode_dir_node() -> Vec<u8> {
    vec![0]
}

pub fn encode_value_node(encoded_value: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + encoded_value.len());
    v.push(1);
    v.extend_from_slice(encoded_value);
    v
}

pub fn parse_node(bytes: &[u8]) -> Result<NodeRecord, String> {
    if bytes.is_empty() {
        return Err("empty node".into());
    }
    match bytes[0] {
        0 => Ok(NodeRecord::Dir),
        1 => Ok(NodeRecord::Value(bytes[1..].to_vec())),
        t => Err(format!("unknown node tag {t}")),
    }
}

#[derive(Debug, Clone)]
pub enum NodeRecord {
    Dir,
    Value(Vec<u8>),
}
