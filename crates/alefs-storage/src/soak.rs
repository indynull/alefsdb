//! Load + compact soak tests (P5 hardening).

#[cfg(test)]
mod tests {
    use crate::{WalStorage, WriteBatch, Storage};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "alefs-soak-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn many_writes_then_compact_bounds_wal() {
        let dir = tmp();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            for i in 0..2_000 {
                let mut b = WriteBatch::new();
                b.put(format!("k{i}").into_bytes(), format!("v{i}").into_bytes());
                s.commit(b).unwrap();
            }
            let wal_before = fs::metadata(dir.join("wal.log")).unwrap().len();
            assert!(wal_before > 10_000, "expected large WAL before compact");
            s.compact().unwrap();
            let wal_after = fs::metadata(dir.join("wal.log")).unwrap().len();
            assert_eq!(wal_after, 0, "WAL truncated after compact");
            assert!(dir.join("checkpoint.bin").exists());
            assert_eq!(
                s.get(b"k1999").unwrap().as_deref(),
                Some(b"v1999".as_ref())
            );
        }
        // reopen
        let s = WalStorage::open(&dir).unwrap();
        assert_eq!(s.get(b"k0").unwrap().as_deref(), Some(b"v0".as_ref()));
        assert_eq!(
            s.get(b"k1999").unwrap().as_deref(),
            Some(b"v1999".as_ref())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_checksum_stops_replay_without_losing_prior() {
        use std::fs::OpenOptions;
        use std::io::Write;
        let dir = tmp();
        {
            let mut s = WalStorage::open(&dir).unwrap();
            s.set_auto_compact_threshold(0);
            let mut b = WriteBatch::new();
            b.put(b"good", b"1");
            s.commit(b).unwrap();
        }
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.join("wal.log"))
                .unwrap();
            // Valid-looking header with wrong checksum body
            f.write_all(b"ALEF").unwrap();
            f.write_all(&[1]).unwrap(); // version
            f.write_all(&5u32.to_le_bytes()).unwrap();
            f.write_all(&[1, 0, 0, 0, 0]).unwrap(); // body
            f.write_all(&0u32.to_le_bytes()).unwrap(); // bad crc
            f.sync_all().unwrap();
        }
        let s = WalStorage::open(&dir).unwrap();
        assert_eq!(s.get(b"good").unwrap().as_deref(), Some(b"1".as_ref()));
        let _ = fs::remove_dir_all(&dir);
    }
}
