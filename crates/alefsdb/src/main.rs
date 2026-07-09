use alefs_namespace::Database;
use alefs_query::execute;
use alefs_storage::WalStorage;
use alefs_types::{DbPath, Scalar, Value};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "alefsdb", about = "Typed structure DB + filesystem")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Create a directory entry (parents must exist)
    Mkdir {
        #[arg(long)]
        data: PathBuf,
        path: String,
    },
    /// Set a value at path
    Set {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
    },
    /// Get a value or directory metadata
    Get {
        #[arg(long)]
        data: PathBuf,
        path: String,
    },
    /// List directory children
    Ls {
        #[arg(long)]
        data: PathBuf,
        #[arg(default_value = "/")]
        path: String,
    },
    /// Delete empty dir or value
    Rm {
        #[arg(long)]
        data: PathBuf,
        path: String,
    },
    /// Hash field set
    Hset {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
    },
    /// List push
    Lpush {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
    },
    /// Set add
    Sadd {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
    },
    /// Tree set (int or string key)
    Tset {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
    },
    /// Run AlefQL query
    Query {
        #[arg(long)]
        data: PathBuf,
        query: String,
    },
    /// Compact WAL (S2)
    Compact {
        #[arg(long)]
        data: PathBuf,
    },
    /// Export namespace as JSON text
    Export {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Import JSON object into namespace
    Import {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        file: PathBuf,
    },
    /// Mount database via FUSE (blocking)
    Serve {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        mount: PathBuf,
    },
}

fn open_db(data: &PathBuf) -> Result<Database<WalStorage>, String> {
    let store = WalStorage::open(data).map_err(|e| e.to_string())?;
    Database::open(store).map_err(|e| e.to_string())
}

fn parse_value(ty: &str, raw: &str) -> Result<Value, String> {
    match ty {
        "null" => Ok(Value::null()),
        "bool" => Ok(Value::bool(raw.parse().map_err(|e| format!("{e}"))?)),
        "int" => Ok(Value::int(raw.parse().map_err(|e| format!("{e}"))?)),
        "float" => Ok(Value::float(raw.parse().map_err(|e| format!("{e}"))?)),
        "string" => Ok(Value::string(raw)),
        "bytes" => Ok(Value::bytes(raw.as_bytes().to_vec())),
        "hash" => Ok(Value::Hash(BTreeMap::new())),
        "list" => Ok(Value::List(Vec::new())),
        "set" => Ok(Value::Set(Vec::new())),
        "tree" => Ok(Value::Tree(BTreeMap::new())),
        other => Err(format!("unknown type {other}")),
    }
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Scalar(Scalar::Null) => "null".into(),
        Value::Scalar(Scalar::Bool(b)) => b.to_string(),
        Value::Scalar(Scalar::Int(n)) => n.to_string(),
        Value::Scalar(Scalar::Float(bits)) => f64::from_bits(*bits).to_string(),
        Value::Scalar(Scalar::String(s)) => s.clone(),
        Value::Scalar(Scalar::Bytes(b)) => format!("bytes[{}]", b.len()),
        Value::Hash(m) => format!("hash({{{}}})", m.len()),
        Value::Set(m) => format!("set({})", m.len()),
        Value::List(m) => format!("list[{}]", m.len()),
        Value::Tree(m) => format!("tree({{{}}})", m.len()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Mkdir { data, path } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            db.mkdir(&p).map_err(|e| e.to_string())?;
            println!("ok {}", p.as_str());
        }
        Cmd::Set {
            data,
            path,
            r#type,
            value,
        } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let v = parse_value(&r#type, &value)?;
            db.set(&p, v).map_err(|e| e.to_string())?;
            println!("ok {}", p.as_str());
        }
        Cmd::Get { data, path } => {
            let db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let e = db.get(&p).map_err(|e| e.to_string())?;
            match e.kind {
                alefs_namespace::EntryKind::Directory => println!("directory {}", p.as_str()),
                alefs_namespace::EntryKind::Value => {
                    let v = e.value.unwrap();
                    println!("{} {}", v.typename(), format_value(&v));
                }
            }
        }
        Cmd::Ls { data, path } => {
            let db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            for (name, kind) in db.list(&p).map_err(|e| e.to_string())? {
                let tag = match kind {
                    alefs_namespace::EntryKind::Directory => "dir",
                    alefs_namespace::EntryKind::Value => "val",
                };
                println!("{tag}\t{name}");
            }
        }
        Cmd::Rm { data, path } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            db.delete(&p).map_err(|e| e.to_string())?;
            println!("ok {}", p.as_str());
        }
        Cmd::Hset {
            data,
            path,
            key,
            r#type,
            value,
        } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let v = parse_value(&r#type, &value)?;
            db.hash_set(&p, &key, v).map_err(|e| e.to_string())?;
            println!("ok");
        }
        Cmd::Lpush {
            data,
            path,
            r#type,
            value,
        } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let v = parse_value(&r#type, &value)?;
            db.list_push(&p, v).map_err(|e| e.to_string())?;
            println!("ok");
        }
        Cmd::Sadd {
            data,
            path,
            r#type,
            value,
        } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let v = parse_value(&r#type, &value)?;
            db.set_add(&p, v).map_err(|e| e.to_string())?;
            println!("ok");
        }
        Cmd::Tset {
            data,
            path,
            key,
            r#type,
            value,
        } => {
            let mut db = open_db(&data)?;
            let p = DbPath::parse(&path).map_err(|e| e.to_string())?;
            let sk = if let Ok(n) = key.parse::<i64>() {
                Scalar::Int(n)
            } else {
                Scalar::String(key)
            };
            let v = parse_value(&r#type, &value)?;
            db.tree_set(&p, sk, v).map_err(|e| e.to_string())?;
            println!("ok");
        }
        Cmd::Query { data, query } => {
            let db = open_db(&data)?;
            let hits = execute(&db, &query).map_err(|e| e.to_string())?;
            for h in hits {
                println!("{}\t{}", h.path.as_str(), h.type_name);
            }
        }
        Cmd::Compact { data } => {
            let mut store = WalStorage::open(&data).map_err(|e| e.to_string())?;
            store.compact().map_err(|e| e.to_string())?;
            println!("ok compacted");
        }
        Cmd::Export { data, out } => {
            let db = open_db(&data)?;
            let json = db.export_json().map_err(|e| e.to_string())?;
            if let Some(path) = out {
                std::fs::write(path, json).map_err(|e| e.to_string())?;
            } else {
                println!("{json}");
            }
        }
        Cmd::Import { data, file } => {
            let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            let mut db = open_db(&data)?;
            db.import_json(&text).map_err(|e| e.to_string())?;
            println!("ok imported");
        }
        Cmd::Serve { data, mount } => {
            println!(
                "mounting {} at {} (ctrl-c to stop)",
                data.display(),
                mount.display()
            );
            alefs_fuse::mount(&data, &mount)?;
        }
    }
    Ok(())
}
