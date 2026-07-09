//! S1 WAL + in-memory index, S2 compaction (checkpoint + truncate).

use crate::batch::{BatchOp, WriteBatch};
use crate::error::StorageError;
use crate::Storage;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"ALEF";
const WAL_VERSION: u8 = 1;
const REC_COMMIT: u8 = 1;
const OP_PUT: u8 = 1;
const OP_DELETE: u8 = 2;
const CHECKPOINT_MAGIC: &[u8; 4] = b"CKPT";
const CHECKPOINT_VERSION: u8 = 1;

/// Durable storage: WAL on disk, primary index in memory.
pub struct WalStorage {
    dir: PathBuf,
    map: BTreeMap<Vec<u8>, Vec<u8>>,
    wal: File,
    /// Bytes written to current WAL since last compaction (approx).
    wal_bytes: u64,
    /// Auto-compact when WAL exceeds this size (0 = never auto).
    auto_compact_threshold: u64,
}

impl WalStorage {
    /// Open or create a store under `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, StorageError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        let wal_path = dir.join("wal.log");

        let mut map = BTreeMap::new();
        let mut wal_start = 0u64;

        let ckpt_path = dir.join("checkpoint.bin");
        if ckpt_path.exists() {
            let (loaded, offset) = load_checkpoint(&ckpt_path)?;
            map = loaded;
            wal_start = offset;
        }

        // Replay WAL from wal_start
        if wal_path.exists() {
            replay_wal(&wal_path, wal_start, &mut map)?;
        }

        let wal = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&wal_path)?;

        let wal_bytes = wal.metadata()?.len().saturating_sub(wal_start);

        Ok(Self {
            dir,
            map,
            wal,
            wal_bytes,
            auto_compact_threshold: 8 * 1024 * 1024, // 8 MiB
        })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn set_auto_compact_threshold(&mut self, bytes: u64) {
        self.auto_compact_threshold = bytes;
    }

    /// S2: write checkpoint of live state and truncate WAL.
    pub fn compact(&mut self) -> Result<(), StorageError> {
        let ckpt_path = self.dir.join("checkpoint.bin");
        let tmp_path = self.dir.join("checkpoint.bin.tmp");
        write_checkpoint(&tmp_path, &self.map, 0)?;
        fs::rename(&tmp_path, &ckpt_path)?;

        // Truncate WAL to empty; checkpoint offset 0 means full state in ckpt.
        self.wal.flush()?;
        self.wal.set_len(0)?;
        self.wal.seek(SeekFrom::Start(0))?;
        // Rewrite checkpoint with wal offset 0 (already).
        self.wal_bytes = 0;

        // fsync dir-ish: sync checkpoint file
        let f = File::open(&ckpt_path)?;
        f.sync_all()?;
        self.wal.sync_all()?;
        Ok(())
    }
}

impl Storage for WalStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.map.get(key).cloned())
    }

    fn commit(&mut self, batch: WriteBatch) -> Result<(), StorageError> {
        if batch.is_empty() {
            return Ok(());
        }
        let record = encode_commit_record(&batch.ops);
        self.wal.write_all(&record)?;
        self.wal.sync_data()?;
        self.wal_bytes += record.len() as u64;

        apply_ops(&mut self.map, &batch.ops);

        if self.auto_compact_threshold > 0 && self.wal_bytes >= self.auto_compact_threshold {
            self.compact()?;
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

fn apply_ops(map: &mut BTreeMap<Vec<u8>, Vec<u8>>, ops: &[BatchOp]) {
    for op in ops {
        match op {
            BatchOp::Put { key, value } => {
                map.insert(key.clone(), value.clone());
            }
            BatchOp::Delete { key } => {
                map.remove(key);
            }
        }
    }
}

fn encode_commit_record(ops: &[BatchOp]) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(REC_COMMIT);
    body.extend_from_slice(&(ops.len() as u32).to_le_bytes());
    for op in ops {
        match op {
            BatchOp::Put { key, value } => {
                body.push(OP_PUT);
                body.extend_from_slice(&(key.len() as u32).to_le_bytes());
                body.extend_from_slice(key);
                body.extend_from_slice(&(value.len() as u32).to_le_bytes());
                body.extend_from_slice(value);
            }
            BatchOp::Delete { key } => {
                body.push(OP_DELETE);
                body.extend_from_slice(&(key.len() as u32).to_le_bytes());
                body.extend_from_slice(key);
            }
        }
    }
    let checksum = crc32(&body);

    let mut rec = Vec::new();
    rec.extend_from_slice(MAGIC);
    rec.push(WAL_VERSION);
    rec.extend_from_slice(&(body.len() as u32).to_le_bytes());
    rec.extend_from_slice(&body);
    rec.extend_from_slice(&checksum.to_le_bytes());
    rec
}

fn replay_wal(
    path: &Path,
    start: u64,
    map: &mut BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<(), StorageError> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    if start > len {
        return Err(StorageError::Corrupt(format!(
            "checkpoint wal offset {start} beyond file len {len}"
        )));
    }
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;

    let mut i = 0usize;
    while i < buf.len() {
        // Need at least magic+ver+len = 4+1+4 = 9
        if buf.len() - i < 9 {
            // Truncated tail — ignore incomplete record (crash mid-write).
            break;
        }
        if &buf[i..i + 4] != MAGIC {
            return Err(StorageError::Corrupt(format!(
                "bad WAL magic at offset {}",
                start + i as u64
            )));
        }
        let version = buf[i + 4];
        if version != WAL_VERSION {
            return Err(StorageError::Corrupt(format!(
                "unsupported WAL version {version}"
            )));
        }
        let body_len = u32::from_le_bytes(buf[i + 5..i + 9].try_into().unwrap()) as usize;
        let rec_end = i + 9 + body_len + 4;
        if rec_end > buf.len() {
            // Incomplete record at end — drop it.
            break;
        }
        let body = &buf[i + 9..i + 9 + body_len];
        let expect = u32::from_le_bytes(buf[i + 9 + body_len..rec_end].try_into().unwrap());
        if crc32(body) != expect {
            // Corrupt/incomplete — stop (do not apply).
            break;
        }
        apply_record_body(body, map)?;
        i = rec_end;
    }
    Ok(())
}

fn apply_record_body(
    body: &[u8],
    map: &mut BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<(), StorageError> {
    if body.is_empty() {
        return Err(StorageError::Corrupt("empty WAL body".into()));
    }
    if body[0] != REC_COMMIT {
        return Err(StorageError::Corrupt(format!(
            "unknown record type {}",
            body[0]
        )));
    }
    let mut c = &body[1..];
    let n = read_u32(&mut c)? as usize;
    let mut ops = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = read_u8(&mut c)?;
        match tag {
            OP_PUT => {
                let k = read_bytes(&mut c)?;
                let v = read_bytes(&mut c)?;
                ops.push(BatchOp::Put { key: k, value: v });
            }
            OP_DELETE => {
                let k = read_bytes(&mut c)?;
                ops.push(BatchOp::Delete { key: k });
            }
            other => {
                return Err(StorageError::Corrupt(format!("unknown op tag {other}")));
            }
        }
    }
    if !c.is_empty() {
        return Err(StorageError::Corrupt("trailing bytes in WAL body".into()));
    }
    apply_ops(map, &ops);
    Ok(())
}

fn write_checkpoint(
    path: &Path,
    map: &BTreeMap<Vec<u8>, Vec<u8>>,
    wal_offset: u64,
) -> Result<(), StorageError> {
    let mut body = Vec::new();
    body.extend_from_slice(&wal_offset.to_le_bytes());
    body.extend_from_slice(&(map.len() as u32).to_le_bytes());
    for (k, v) in map {
        body.extend_from_slice(&(k.len() as u32).to_le_bytes());
        body.extend_from_slice(k);
        body.extend_from_slice(&(v.len() as u32).to_le_bytes());
        body.extend_from_slice(v);
    }
    let checksum = crc32(&body);

    let mut out = Vec::new();
    out.extend_from_slice(CHECKPOINT_MAGIC);
    out.push(CHECKPOINT_VERSION);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&checksum.to_le_bytes());

    let mut f = File::create(path)?;
    f.write_all(&out)?;
    f.sync_all()?;
    Ok(())
}

type CheckpointState = (BTreeMap<Vec<u8>, Vec<u8>>, u64);

fn load_checkpoint(path: &Path) -> Result<CheckpointState, StorageError> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if buf.len() < 9 {
        return Err(StorageError::Corrupt("checkpoint too short".into()));
    }
    if &buf[0..4] != CHECKPOINT_MAGIC {
        return Err(StorageError::Corrupt("bad checkpoint magic".into()));
    }
    if buf[4] != CHECKPOINT_VERSION {
        return Err(StorageError::Corrupt(format!(
            "unsupported checkpoint version {}",
            buf[4]
        )));
    }
    let body_len = u32::from_le_bytes(buf[5..9].try_into().unwrap()) as usize;
    if buf.len() < 9 + body_len + 4 {
        return Err(StorageError::Corrupt("checkpoint truncated".into()));
    }
    let body = &buf[9..9 + body_len];
    let expect = u32::from_le_bytes(buf[9 + body_len..9 + body_len + 4].try_into().unwrap());
    if crc32(body) != expect {
        return Err(StorageError::Corrupt("checkpoint checksum mismatch".into()));
    }
    let mut c = body;
    let wal_offset = u64::from_le_bytes(read_array(&mut c)?);
    let n = read_u32(&mut c)? as usize;
    let mut map = BTreeMap::new();
    for _ in 0..n {
        let k = read_bytes(&mut c)?;
        let v = read_bytes(&mut c)?;
        map.insert(k, v);
    }
    if !c.is_empty() {
        return Err(StorageError::Corrupt("checkpoint trailing bytes".into()));
    }
    Ok((map, wal_offset))
}

fn read_u8(cur: &mut &[u8]) -> Result<u8, StorageError> {
    if cur.is_empty() {
        return Err(StorageError::Corrupt("unexpected eof".into()));
    }
    let b = cur[0];
    *cur = &cur[1..];
    Ok(b)
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, StorageError> {
    Ok(u32::from_le_bytes(read_array(cur)?))
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], StorageError> {
    if cur.len() < N {
        return Err(StorageError::Corrupt("unexpected eof".into()));
    }
    let mut a = [0u8; N];
    a.copy_from_slice(&cur[..N]);
    *cur = &cur[N..];
    Ok(a)
}

fn read_bytes(cur: &mut &[u8]) -> Result<Vec<u8>, StorageError> {
    let len = read_u32(cur)? as usize;
    if cur.len() < len {
        return Err(StorageError::Corrupt("unexpected eof".into()));
    }
    let v = cur[..len].to_vec();
    *cur = &cur[len..];
    Ok(v)
}

/// Small CRC32 (IEEE) — no external deps.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB88320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "alefs-wal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn durable_across_reopen() {
        let dir = tmpdir();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            let mut b = WriteBatch::new();
            b.put(b"k", b"v");
            s.commit(b).unwrap();
        }
        {
            let s = WalStorage::open(&dir).unwrap();
            assert_eq!(s.get(b"k").unwrap().as_deref(), Some(b"v".as_ref()));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_tail_does_not_apply() {
        let dir = tmpdir();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            let mut b = WriteBatch::new();
            b.put(b"ok", b"1");
            s.commit(b).unwrap();
        }
        // Append garbage / truncated record
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.join("wal.log"))
                .unwrap();
            f.write_all(b"ALEF").unwrap();
            f.write_all(&[1, 0, 0, 0, 50]).unwrap(); // claims huge body
            f.sync_all().unwrap();
        }
        let s = WalStorage::open(&dir).unwrap();
        assert_eq!(s.get(b"ok").unwrap().as_deref(), Some(b"1".as_ref()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_rewrites_and_survives() {
        let dir = tmpdir();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            for i in 0..20 {
                let mut b = WriteBatch::new();
                b.put(format!("k{i}").into_bytes(), b"x".to_vec());
                s.commit(b).unwrap();
            }
            // overwrite
            let mut b = WriteBatch::new();
            b.put(b"k0", b"final");
            b.delete(b"k1");
            s.commit(b).unwrap();
            s.compact().unwrap();
            assert!(s.wal_bytes == 0 || s.wal.metadata().unwrap().len() == 0);
        }
        let s = WalStorage::open(&dir).unwrap();
        assert_eq!(s.get(b"k0").unwrap().as_deref(), Some(b"final".as_ref()));
        assert_eq!(s.get(b"k1").unwrap(), None);
        assert_eq!(s.get(b"k2").unwrap().as_deref(), Some(b"x".as_ref()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_persists() {
        let dir = tmpdir();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            let mut b = WriteBatch::new();
            b.put(b"a", b"1");
            s.commit(b).unwrap();
            let mut b = WriteBatch::new();
            b.delete(b"a");
            s.commit(b).unwrap();
        }
        let s = WalStorage::open(&dir).unwrap();
        assert_eq!(s.get(b"a").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
