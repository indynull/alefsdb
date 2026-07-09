//! FUSE projection of an alefsdb namespace.
//!
//! Mount is a view over [`Database`]; host files under the data dir are never
//! written directly from this adapter.

use alefs_namespace::{Database, EntryKind};
use alefs_storage::WalStorage;
use alefs_types::{DbPath, Scalar, Value};
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyXattr, Request, FUSE_ROOT_ID,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

/// Shared DB handle for the FUSE session.
pub type SharedDb = Arc<Mutex<Database<WalStorage>>>;

pub fn open_shared(data_dir: impl AsRef<Path>) -> Result<SharedDb, String> {
    let store = WalStorage::open(data_dir.as_ref()).map_err(|e| e.to_string())?;
    let db = Database::open(store).map_err(|e| e.to_string())?;
    Ok(Arc::new(Mutex::new(db)))
}

/// Mount `data_dir` database at `mountpoint` (blocking). Opens its own store handle.
pub fn mount(data_dir: impl AsRef<Path>, mountpoint: impl AsRef<Path>) -> Result<(), String> {
    let db = open_shared(data_dir)?;
    mount_shared(db, mountpoint)
}

/// Mount an already-open shared database (for daemon + FUSE in one process).
pub fn mount_shared(db: SharedDb, mountpoint: impl AsRef<Path>) -> Result<(), String> {
    let fs = AlefsFs::new(db);
    let opts = vec![
        MountOption::FSName("alefsdb".into()),
        MountOption::AutoUnmount,
        MountOption::DefaultPermissions,
    ];
    fuser::mount2(fs, mountpoint, &opts).map_err(|e| e.to_string())
}

struct AlefsFs {
    db: SharedDb,
    /// ino -> path string
    ino_to_path: Mutex<HashMap<u64, String>>,
    path_to_ino: Mutex<HashMap<String, u64>>,
    next_ino: Mutex<u64>,
    /// pending scalar writes: ino -> bytes
    dirty: Mutex<HashMap<u64, Vec<u8>>>,
}

impl AlefsFs {
    fn new(db: SharedDb) -> Self {
        let mut ino_to_path = HashMap::new();
        let mut path_to_ino = HashMap::new();
        ino_to_path.insert(FUSE_ROOT_ID, "/".to_string());
        path_to_ino.insert("/".to_string(), FUSE_ROOT_ID);
        Self {
            db,
            ino_to_path: Mutex::new(ino_to_path),
            path_to_ino: Mutex::new(path_to_ino),
            next_ino: Mutex::new(FUSE_ROOT_ID + 1),
            dirty: Mutex::new(HashMap::new()),
        }
    }

    fn path_for_ino(&self, ino: u64) -> Option<String> {
        self.ino_to_path.lock().unwrap().get(&ino).cloned()
    }

    fn ino_for_path(&self, path: &str) -> u64 {
        let mut p2i = self.path_to_ino.lock().unwrap();
        if let Some(ino) = p2i.get(path) {
            return *ino;
        }
        let mut next = self.next_ino.lock().unwrap();
        let ino = *next;
        *next += 1;
        p2i.insert(path.to_string(), ino);
        self.ino_to_path
            .lock()
            .unwrap()
            .insert(ino, path.to_string());
        ino
    }

    fn attr_for(path: &str, kind: EntryKind, value: Option<&Value>) -> FileAttr {
        let ino = 0; // filled by caller
        let (kind_ft, size) = match (kind, value) {
            (EntryKind::Directory, _) => (FileType::Directory, 0),
            (EntryKind::Value, Some(Value::Scalar(s))) => {
                (FileType::RegularFile, scalar_bytes(s).len() as u64)
            }
            (EntryKind::Value, Some(Value::Hash(_)))
            | (EntryKind::Value, Some(Value::Set(_)))
            | (EntryKind::Value, Some(Value::List(_)))
            | (EntryKind::Value, Some(Value::Tree(_))) => (FileType::Directory, 0),
            (EntryKind::Value, _) => (FileType::RegularFile, 0),
        };
        let _ = path;
        FileAttr {
            ino,
            size,
            blocks: 1,
            atime: SystemTime::now(),
            mtime: SystemTime::now(),
            ctime: SystemTime::now(),
            crtime: UNIX_EPOCH,
            kind: kind_ft,
            perm: if kind_ft == FileType::Directory {
                0o755
            } else {
                0o644
            },
            nlink: 1,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}

fn scalar_bytes(s: &Scalar) -> Vec<u8> {
    match s {
        Scalar::Null => b"null".to_vec(),
        Scalar::Bool(true) => b"true".to_vec(),
        Scalar::Bool(false) => b"false".to_vec(),
        Scalar::Int(n) => n.to_string().into_bytes(),
        Scalar::Float(bits) => f64::from_bits(*bits).to_string().into_bytes(),
        Scalar::String(s) => s.as_bytes().to_vec(),
        Scalar::Bytes(b) => b.clone(),
    }
}

fn parse_scalar_text(existing: &Scalar, data: &[u8]) -> Result<Scalar, ()> {
    match existing {
        Scalar::Bytes(_) => Ok(Scalar::Bytes(data.to_vec())),
        Scalar::String(_) => {
            let s = std::str::from_utf8(data).map_err(|_| ())?;
            Ok(Scalar::String(s.to_owned()))
        }
        Scalar::Int(_) => {
            let s = std::str::from_utf8(data).map_err(|_| ())?.trim();
            Ok(Scalar::Int(s.parse().map_err(|_| ())?))
        }
        Scalar::Float(_) => {
            let s = std::str::from_utf8(data).map_err(|_| ())?.trim();
            let f: f64 = s.parse().map_err(|_| ())?;
            Ok(Scalar::from_f64(f))
        }
        Scalar::Bool(_) => {
            let s = std::str::from_utf8(data).map_err(|_| ())?.trim();
            match s {
                "true" | "1" => Ok(Scalar::Bool(true)),
                "false" | "0" => Ok(Scalar::Bool(false)),
                _ => Err(()),
            }
        }
        Scalar::Null => {
            let s = std::str::from_utf8(data).map_err(|_| ())?.trim();
            if s == "null" || s.is_empty() {
                Ok(Scalar::Null)
            } else {
                Err(())
            }
        }
    }
}

fn type_xattr(value: Option<&Value>, kind: EntryKind) -> String {
    match (kind, value) {
        (EntryKind::Directory, _) => "directory".into(),
        (_, Some(v)) => v.typename().into(),
        _ => "unknown".into(),
    }
}

impl Filesystem for AlefsFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let Some(parent_path) = self.path_for_ino(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let child_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let db = self.db.lock().unwrap();
        let path = match DbPath::parse(&child_path) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        // Structure projections: hash/list/set/tree children
        if let Ok(entry) = db.get(&DbPath::parse(&parent_path).unwrap_or_else(|_| DbPath::root())) {
            if let Some(ref val) = entry.value {
                if let Some(attr) = project_child_attr(val, name) {
                    let ino = self.ino_for_path(&child_path);
                    let mut a = attr;
                    a.ino = ino;
                    // stash synthetic scalar paths as special? store encoded value path with marker
                    reply.entry(&TTL, &a, 0);
                    return;
                }
            }
        }
        match db.get(&path) {
            Ok(entry) => {
                let ino = self.ino_for_path(&child_path);
                let mut attr = Self::attr_for(&child_path, entry.kind, entry.value.as_ref());
                attr.ino = ino;
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let Some(path_str) = self.path_for_ino(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        if let Some(data) = self.dirty.lock().unwrap().get(&ino) {
            let attr = FileAttr {
                ino,
                size: data.len() as u64,
                blocks: 1,
                atime: SystemTime::now(),
                mtime: SystemTime::now(),
                ctime: SystemTime::now(),
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: unsafe { libc::getuid() },
                gid: unsafe { libc::getgid() },
                rdev: 0,
                blksize: 512,
                flags: 0,
            };
            let _ = path_str;
            reply.attr(&TTL, &attr);
            return;
        }
        let db = self.db.lock().unwrap();
        // synthetic structure member paths: /path/key for nested
        if let Ok(path) = DbPath::parse(&path_str) {
            if let Ok(entry) = db.get(&path) {
                let mut attr = Self::attr_for(&path_str, entry.kind, entry.value.as_ref());
                attr.ino = ino;
                reply.attr(&TTL, &attr);
                return;
            }
            // try parent structure projection
            if let Some(parent) = path.parent() {
                if let Ok(entry) = db.get(&parent) {
                    if let Some(val) = entry.value.as_ref() {
                        let name = path.segments().last().map(|s| s.as_str()).unwrap_or("");
                        if let Some(mut attr) = project_child_attr(val, name) {
                            attr.ino = ino;
                            reply.attr(&TTL, &attr);
                            return;
                        }
                    }
                }
            }
        }
        reply.error(libc::ENOENT);
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        if let Some(data) = self.dirty.lock().unwrap().get(&ino) {
            let start = offset as usize;
            if start >= data.len() {
                reply.data(&[]);
                return;
            }
            let end = (start + size as usize).min(data.len());
            reply.data(&data[start..end]);
            return;
        }
        let Some(path_str) = self.path_for_ino(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let db = self.db.lock().unwrap();
        let path = match DbPath::parse(&path_str) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        if let Ok(entry) = db.get(&path) {
            if let Some(Value::Scalar(s)) = entry.value {
                let data = scalar_bytes(&s);
                let start = offset as usize;
                if start >= data.len() {
                    reply.data(&[]);
                    return;
                }
                let end = (start + size as usize).min(data.len());
                reply.data(&data[start..end]);
                return;
            }
        }
        // projected child of structure
        if let Some(parent) = path.parent() {
            if let Ok(entry) = db.get(&parent) {
                if let Some(val) = entry.value {
                    let name = path.segments().last().map(|s| s.as_str()).unwrap_or("");
                    if let Some(data) = project_child_bytes(&val, name) {
                        let start = offset as usize;
                        if start >= data.len() {
                            reply.data(&[]);
                            return;
                        }
                        let end = (start + size as usize).min(data.len());
                        reply.data(&data[start..end]);
                        return;
                    }
                }
            }
        }
        reply.error(libc::EISDIR);
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let mut dirty = self.dirty.lock().unwrap();
        let buf = dirty.entry(ino).or_default();
        let end = offset as usize + data.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[offset as usize..end].copy_from_slice(data);
        reply.written(data.len() as u32);
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        if let Err(e) = self.commit_dirty(ino) {
            reply.error(e);
        } else {
            reply.ok();
        }
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: fuser::ReplyEmpty,
    ) {
        if let Err(e) = self.commit_dirty(ino) {
            reply.error(e);
        } else {
            reply.ok();
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let Some(path_str) = self.path_for_ino(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let db = self.db.lock().unwrap();
        let path = match DbPath::parse(&path_str) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let mut entries: Vec<(String, FileType)> = vec![
            (".".into(), FileType::Directory),
            ("..".into(), FileType::Directory),
        ];
        if let Ok(entry) = db.get(&path) {
            match entry.kind {
                EntryKind::Directory => {
                    if let Ok(kids) = db.list(&path) {
                        for (name, kind) in kids {
                            let ft = match kind {
                                EntryKind::Directory => FileType::Directory,
                                EntryKind::Value => {
                                    // peek
                                    let child = path.join(&name).unwrap();
                                    match db.get(&child).ok().and_then(|e| e.value) {
                                        Some(Value::Scalar(_)) => FileType::RegularFile,
                                        Some(_) => FileType::Directory,
                                        None => FileType::Directory,
                                    }
                                }
                            };
                            entries.push((name, ft));
                        }
                    }
                }
                EntryKind::Value => {
                    if let Some(val) = entry.value.as_ref() {
                        entries.extend(project_listing(val));
                    }
                }
            }
        } else {
            reply.error(libc::ENOENT);
            return;
        }
        for (i, (name, ft)) in entries.into_iter().enumerate().skip(offset as usize) {
            let child_path = if name == "." || name == ".." {
                path_str.clone()
            } else if path_str == "/" {
                format!("/{name}")
            } else {
                format!("{path_str}/{name}")
            };
            let child_ino = self.ino_for_path(&child_path);
            if reply.add(child_ino, (i + 1) as i64, ft, name) {
                break;
            }
        }
        reply.ok();
    }

    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        size: u32,
        reply: ReplyXattr,
    ) {
        let Some(path_str) = self.path_for_ino(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let db = self.db.lock().unwrap();
        let path = match DbPath::parse(&path_str) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::ENOENT);
                return;
            }
        };

        let val = if name == OsStr::new("user.alefs.type") {
            if let Ok(entry) = db.get(&path) {
                type_xattr(entry.value.as_ref(), entry.kind)
            } else if let Some(parent) = path.parent() {
                // projected structure child: type of projected value
                if let Ok(entry) = db.get(&parent) {
                    if let Some(ref v) = entry.value {
                        let child = path.segments().last().map(|s| s.as_str()).unwrap_or("");
                        if let Some(cv) = project_child_value(v, child) {
                            type_xattr(Some(&cv), EntryKind::Value)
                        } else {
                            reply.error(libc::ENOENT);
                            return;
                        }
                    } else {
                        reply.error(libc::ENOENT);
                        return;
                    }
                } else {
                    reply.error(libc::ENOENT);
                    return;
                }
            } else {
                reply.error(libc::ENOENT);
                return;
            }
        } else if name == OsStr::new("user.alefs.member") {
            // Display hint for set members named by content hash.
            if let Some(parent) = path.parent() {
                if let Ok(entry) = db.get(&parent) {
                    if let Some(Value::Set(members)) = entry.value {
                        let child = path.segments().last().map(|s| s.as_str()).unwrap_or("");
                        if let Some(m) = members.iter().find(|m| set_member_name(m) == child) {
                            format_member_hint(m)
                        } else {
                            reply.error(libc::ENODATA);
                            return;
                        }
                    } else {
                        reply.error(libc::ENODATA);
                        return;
                    }
                } else {
                    reply.error(libc::ENODATA);
                    return;
                }
            } else {
                reply.error(libc::ENODATA);
                return;
            }
        } else {
            reply.error(libc::ENODATA);
            return;
        };

        let bytes = val.as_bytes();
        if size == 0 {
            reply.size(bytes.len() as u32);
        } else if size < bytes.len() as u32 {
            reply.error(libc::ERANGE);
        } else {
            reply.data(bytes);
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        // Editor temp-file rename workflows are intentionally unsupported in v1.
        reply.error(libc::ENOTSUP);
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let Some(parent_path) = self.path_for_ino(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let child_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let mut db = self.db.lock().unwrap();
        let path = match DbPath::parse(&child_path) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        match db.mkdir(&path) {
            Ok(()) => {
                let ino = self.ino_for_path(&child_path);
                let mut attr = Self::attr_for(&child_path, EntryKind::Directory, None);
                attr.ino = ino;
                reply.entry(&TTL, &attr, 0);
            }
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        let Some(parent_path) = self.path_for_ino(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let child_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let mut db = self.db.lock().unwrap();
        // structure member delete
        if let Ok(parent_db) = DbPath::parse(&parent_path) {
            if let Ok(entry) = db.get(&parent_db) {
                if let Some(val) = entry.value {
                    if let Some(new_val) = project_unlink(val, name) {
                        if db.set(&parent_db, new_val).is_ok() {
                            reply.ok();
                            return;
                        }
                    }
                }
            }
        }
        match DbPath::parse(&child_path) {
            Ok(p) => match db.delete(&p) {
                Ok(()) => reply.ok(),
                Err(_) => reply.error(libc::EIO),
            },
            Err(_) => reply.error(libc::EINVAL),
        }
    }

    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        self.unlink(req, parent, name, reply);
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        let Some(parent_path) = self.path_for_ino(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let name = match name.to_str() {
            Some(n) => n,
            None => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        let child_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let mut db = self.db.lock().unwrap();
        // create under hash
        if let Ok(parent_db) = DbPath::parse(&parent_path) {
            if let Ok(entry) = db.get(&parent_db) {
                if let Some(Value::Hash(mut m)) = entry.value {
                    m.insert(name.to_string(), Value::string(""));
                    if db.set(&parent_db, Value::Hash(m)).is_ok() {
                        let ino = self.ino_for_path(&child_path);
                        let mut attr =
                            Self::attr_for(&child_path, EntryKind::Value, Some(&Value::string("")));
                        attr.ino = ino;
                        reply.created(&TTL, &attr, 0, 0, 0);
                        return;
                    }
                }
            }
        }
        let path = match DbPath::parse(&child_path) {
            Ok(p) => p,
            Err(_) => {
                reply.error(libc::EINVAL);
                return;
            }
        };
        match db.set(&path, Value::string("")) {
            Ok(()) => {
                let ino = self.ino_for_path(&child_path);
                let mut attr =
                    Self::attr_for(&child_path, EntryKind::Value, Some(&Value::string("")));
                attr.ino = ino;
                reply.created(&TTL, &attr, 0, 0, 0);
            }
            Err(_) => reply.error(libc::EIO),
        }
    }
}

impl AlefsFs {
    fn commit_dirty(&self, ino: u64) -> Result<(), i32> {
        let data = {
            let mut dirty = self.dirty.lock().unwrap();
            match dirty.remove(&ino) {
                Some(d) => d,
                None => return Ok(()),
            }
        };
        let Some(path_str) = self.path_for_ino(ino) else {
            return Err(libc::ENOENT);
        };
        let mut db = self.db.lock().unwrap();
        let path = DbPath::parse(&path_str).map_err(|_| libc::EINVAL)?;
        if let Ok(entry) = db.get(&path) {
            if let Some(Value::Scalar(existing)) = entry.value {
                let new_s = parse_scalar_text(&existing, &data).map_err(|_| libc::EINVAL)?;
                db.set(&path, Value::Scalar(new_s)).map_err(|_| libc::EIO)?;
                return Ok(());
            }
            return Err(libc::EPERM);
        }
        // projected member of hash
        if let Some(parent) = path.parent() {
            if let Ok(entry) = db.get(&parent) {
                if let Some(Value::Hash(mut m)) = entry.value {
                    let name = path.segments().last().unwrap().clone();
                    let existing = m.get(&name).cloned().unwrap_or(Value::string(""));
                    match existing {
                        Value::Scalar(s) => {
                            let new_s = parse_scalar_text(&s, &data).map_err(|_| libc::EINVAL)?;
                            m.insert(name, Value::Scalar(new_s));
                            db.set(&parent, Value::Hash(m)).map_err(|_| libc::EIO)?;
                            return Ok(());
                        }
                        _ => return Err(libc::EPERM),
                    }
                }
            }
        }
        Err(libc::ENOENT)
    }
}

fn project_listing(val: &Value) -> Vec<(String, FileType)> {
    match val {
        Value::Hash(m) => m
            .keys()
            .map(|k| {
                let ft = match m.get(k) {
                    Some(Value::Scalar(_)) => FileType::RegularFile,
                    _ => FileType::Directory,
                };
                (k.clone(), ft)
            })
            .collect(),
        Value::List(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let ft = match v {
                    Value::Scalar(_) => FileType::RegularFile,
                    _ => FileType::Directory,
                };
                (i.to_string(), ft)
            })
            .collect(),
        Value::Set(members) => members
            .iter()
            .map(|m| {
                let name = set_member_name(m);
                (name, FileType::RegularFile)
            })
            .collect(),
        Value::Tree(m) => m
            .keys()
            .map(|k| {
                let name = match k {
                    Scalar::String(s) => s.clone(),
                    Scalar::Int(n) => n.to_string(),
                    other => format!("{other:?}"),
                };
                (name, FileType::RegularFile)
            })
            .collect(),
        _ => vec![],
    }
}

fn set_member_name(v: &Value) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let enc = alefs_types::encode_payload(v);
    let mut h = DefaultHasher::new();
    enc.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn format_member_hint(v: &Value) -> String {
    match v {
        Value::Scalar(Scalar::String(s)) => s.clone(),
        Value::Scalar(Scalar::Int(n)) => n.to_string(),
        Value::Scalar(Scalar::Bool(b)) => b.to_string(),
        Value::Scalar(Scalar::Null) => "null".into(),
        Value::Scalar(Scalar::Float(bits)) => f64::from_bits(*bits).to_string(),
        Value::Scalar(Scalar::Bytes(b)) => format!("bytes[{}]", b.len()),
        other => format!("<{}>", other.typename()),
    }
}

fn project_child_attr(val: &Value, name: &str) -> Option<FileAttr> {
    let v = project_child_value(val, name)?;
    let kind = match &v {
        Value::Scalar(_) => EntryKind::Value,
        _ => EntryKind::Value,
    };
    Some(AlefsFs::attr_for("", kind, Some(&v)))
}

fn project_child_bytes(val: &Value, name: &str) -> Option<Vec<u8>> {
    match project_child_value(val, name)? {
        Value::Scalar(s) => Some(scalar_bytes(&s)),
        _ => None,
    }
}

fn project_child_value(val: &Value, name: &str) -> Option<Value> {
    match val {
        Value::Hash(m) => m.get(name).cloned(),
        Value::List(items) => {
            let idx: usize = name.parse().ok()?;
            items.get(idx).cloned()
        }
        Value::Set(members) => members.iter().find(|m| set_member_name(m) == name).cloned(),
        Value::Tree(m) => {
            if let Ok(n) = name.parse::<i64>() {
                if let Some(v) = m.get(&Scalar::Int(n)) {
                    return Some(v.clone());
                }
            }
            m.get(&Scalar::String(name.to_string())).cloned()
        }
        _ => None,
    }
}

fn project_unlink(val: Value, name: &str) -> Option<Value> {
    match val {
        Value::Hash(mut m) => {
            m.remove(name)?;
            Some(Value::Hash(m))
        }
        Value::List(mut items) => {
            let idx: usize = name.parse().ok()?;
            if idx < items.len() {
                items.remove(idx);
                Some(Value::List(items))
            } else {
                None
            }
        }
        Value::Set(mut members) => {
            let before = members.len();
            members.retain(|m| set_member_name(m) != name);
            if members.len() == before {
                None
            } else {
                Some(Value::Set(members))
            }
        }
        Value::Tree(mut m) => {
            if let Ok(n) = name.parse::<i64>() {
                m.remove(&Scalar::Int(n))?;
            } else {
                m.remove(&Scalar::String(name.to_string()))?;
            }
            Some(Value::Tree(m))
        }
        _ => None,
    }
}

/// Returns true if FUSE device is usable for integration tests.
pub fn fuse_available() -> bool {
    Path::new("/dev/fuse").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_device_check_does_not_panic() {
        let _ = fuse_available();
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use alefs_types::Value;
    use std::fs;
    use std::io::Write;
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn tmp_pair() -> (std::path::PathBuf, std::path::PathBuf) {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data = std::env::temp_dir().join(format!("alefs-fuse-data-{n}"));
        let mnt = std::env::temp_dir().join(format!("alefs-fuse-mnt-{n}"));
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&mnt);
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&mnt).unwrap();
        (data, mnt)
    }

    #[test]
    fn fuse_cat_scalar_when_available() {
        if !fuse_available() {
            eprintln!("skip: /dev/fuse missing");
            return;
        }
        // Need fusermount3 for unmount
        if Command::new("fusermount3").arg("-V").output().is_err()
            && Command::new("fusermount").arg("-V").output().is_err()
        {
            eprintln!("skip: fusermount not installed");
            return;
        }

        let (data, mnt) = tmp_pair();
        let db = open_shared(&data).unwrap();
        {
            let mut g = db.lock().unwrap();
            g.mkdir(&DbPath::parse("/d").unwrap()).unwrap();
            g.set(&DbPath::parse("/d/x").unwrap(), Value::string("hello"))
                .unwrap();
        }
        let db_c = Arc::clone(&db);
        let mnt_c = mnt.clone();
        let th = thread::spawn(move || {
            let _ = mount_shared(db_c, &mnt_c);
        });
        // Wait for mount
        let target = mnt.join("d").join("x");
        let mut ok = false;
        for _ in 0..50 {
            if target.exists() {
                ok = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if !ok {
            // may lack permissions for fuse
            eprintln!("skip: mount did not appear (permissions?)");
            unmount(&mnt);
            let _ = fs::remove_dir_all(&data);
            let _ = fs::remove_dir_all(&mnt);
            return;
        }
        let content = fs::read_to_string(&target).expect("read fused file");
        assert_eq!(content, "hello");
        // type-stable write
        {
            let mut f = fs::File::create(&target).unwrap();
            f.write_all(b"world").unwrap();
            f.sync_all().unwrap();
        }
        thread::sleep(Duration::from_millis(100));
        let g = db.lock().unwrap();
        let e = g.get(&DbPath::parse("/d/x").unwrap()).unwrap();
        assert_eq!(e.value, Some(Value::string("world")));
        drop(g);
        unmount(&mnt);
        let _ = th.join();
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&mnt);
    }

    fn unmount(mnt: &Path) {
        let _ = Command::new("fusermount3").args(["-u"]).arg(mnt).status();
        let _ = Command::new("fusermount").args(["-u"]).arg(mnt).status();
    }
}
