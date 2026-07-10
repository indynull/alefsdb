use alefs_server::{
    default_socket_path, dispatch, open_daemon, open_db, rpc_call, serve_daemon, Request, Response,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "alefsdb", about = "Typed structure DB + filesystem")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the daemon (Unix socket; optional FUSE mount)
    Serve {
        #[arg(long)]
        data: PathBuf,
        /// Unix socket path (default: <data>/alefs.sock)
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Optional FUSE mount point
        #[arg(long)]
        mount: Option<PathBuf>,
    },
    /// Server stats (requires running daemon, or --direct)
    Stats {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        direct: bool,
    },
    /// Atomic multi-op transaction: JSON array of request objects
    Txn {
        #[arg(long)]
        data: PathBuf,
        /// Path to JSON file containing an array of ops
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        direct: bool,
    },
    /// Create a directory entry (parents must exist)
    Mkdir {
        #[arg(long)]
        data: PathBuf,
        path: String,
        /// Force open-local store instead of daemon socket
        #[arg(long)]
        direct: bool,
    },
    Set {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        direct: bool,
    },
    Get {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long)]
        direct: bool,
    },
    Ls {
        #[arg(long)]
        data: PathBuf,
        #[arg(default_value = "/")]
        path: String,
        #[arg(long)]
        direct: bool,
    },
    Rm {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long)]
        direct: bool,
    },
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
        #[arg(long)]
        direct: bool,
    },
    Lpush {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        direct: bool,
    },
    Sadd {
        #[arg(long)]
        data: PathBuf,
        path: String,
        #[arg(long, default_value = "string")]
        r#type: String,
        #[arg(long)]
        value: String,
        #[arg(long)]
        direct: bool,
    },
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
        #[arg(long)]
        direct: bool,
    },
    Query {
        #[arg(long)]
        data: PathBuf,
        query: String,
        #[arg(long)]
        direct: bool,
    },
    Compact {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        direct: bool,
    },
    Export {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        direct: bool,
    },
    Import {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        direct: bool,
    },
}

fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .with_writer(std::io::stderr)
        .try_init();
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::Serve {
            data,
            socket,
            mount,
        } => {
            let sock = socket.unwrap_or_else(|| default_socket_path(&data));
            let daemon = open_daemon(&data).map_err(|e| e.to_string())?;
            if let Some(mnt) = mount {
                let db_fuse = Arc::clone(&daemon.db);
                let mnt_c = mnt.clone();
                thread::spawn(move || {
                    if let Err(e) = alefs_fuse::mount_shared(db_fuse, &mnt_c) {
                        tracing::error!(error = %e, "fuse error");
                    }
                });
                tracing::info!(path = %mnt.display(), "fuse mounting");
            }
            serve_daemon(daemon, sock).map_err(|e| e.to_string())?;
            Ok(())
        }
        Cmd::Stats { data, direct } => print_resp(call(&data, direct, Request::Stats)?),
        Cmd::Txn { data, file, direct } => {
            let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            let ops: Vec<Request> =
                serde_json::from_str(&text).map_err(|e| format!("txn json: {e}"))?;
            print_resp(call(&data, direct, Request::Batch { ops })?)
        }
        Cmd::Mkdir { data, path, direct } => {
            print_resp(call(&data, direct, Request::Mkdir { path })?)
        }
        Cmd::Set {
            data,
            path,
            r#type,
            value,
            direct,
        } => print_resp(call(
            &data,
            direct,
            Request::Set {
                path,
                type_name: r#type,
                value,
            },
        )?),
        Cmd::Get { data, path, direct } => print_resp(call(&data, direct, Request::Get { path })?),
        Cmd::Ls { data, path, direct } => print_resp(call(&data, direct, Request::Ls { path })?),
        Cmd::Rm { data, path, direct } => print_resp(call(&data, direct, Request::Rm { path })?),
        Cmd::Hset {
            data,
            path,
            key,
            r#type,
            value,
            direct,
        } => print_resp(call(
            &data,
            direct,
            Request::Hset {
                path,
                key,
                type_name: r#type,
                value,
            },
        )?),
        Cmd::Lpush {
            data,
            path,
            r#type,
            value,
            direct,
        } => print_resp(call(
            &data,
            direct,
            Request::Lpush {
                path,
                type_name: r#type,
                value,
            },
        )?),
        Cmd::Sadd {
            data,
            path,
            r#type,
            value,
            direct,
        } => print_resp(call(
            &data,
            direct,
            Request::Sadd {
                path,
                type_name: r#type,
                value,
            },
        )?),
        Cmd::Tset {
            data,
            path,
            key,
            r#type,
            value,
            direct,
        } => print_resp(call(
            &data,
            direct,
            Request::Tset {
                path,
                key,
                type_name: r#type,
                value,
            },
        )?),
        Cmd::Query {
            data,
            query,
            direct,
        } => print_resp(call(&data, direct, Request::Query { query })?),
        Cmd::Compact { data, direct } => print_resp(call(&data, direct, Request::Compact)?),
        Cmd::Export { data, out, direct } => {
            let resp = call(&data, direct, Request::Export)?;
            match resp {
                Response::Export { json } => {
                    if let Some(path) = out {
                        std::fs::write(path, json).map_err(|e| e.to_string())?;
                    } else {
                        println!("{json}");
                    }
                    Ok(())
                }
                Response::Err { message } => Err(message),
                other => Err(format!("unexpected response: {other:?}")),
            }
        }
        Cmd::Import { data, file, direct } => {
            let json = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            print_resp(call(&data, direct, Request::Import { json })?)
        }
    }
}

/// Prefer daemon socket when present; otherwise open the store directly.
fn call(data: &PathBuf, direct: bool, req: Request) -> Result<Response, String> {
    let sock = default_socket_path(data);
    if !direct && sock.exists() {
        return rpc_call(&sock, req).map_err(|e| e.to_string());
    }
    // Direct path: open, dispatch once, drop.
    let db = open_db(data).map_err(|e| e.to_string())?;
    Ok(dispatch(&db, req))
}

fn print_resp(resp: Response) -> Result<(), String> {
    match resp {
        Response::Ok { message } => {
            println!("{message}");
            Ok(())
        }
        Response::Value { type_name, display } => {
            println!("{type_name} {display}");
            Ok(())
        }
        Response::List { entries } => {
            for e in entries {
                println!("{}\t{}", e.kind, e.name);
            }
            Ok(())
        }
        Response::Query { hits } => {
            for h in hits {
                println!("{}\t{}", h.path, h.type_name);
            }
            Ok(())
        }
        Response::Export { json } => {
            println!("{json}");
            Ok(())
        }
        Response::Stats {
            uptime_sec,
            requests,
            mutations,
            queries,
            errors,
            keys_approx,
        } => {
            println!("uptime_sec={uptime_sec}");
            println!("requests={requests}");
            println!("mutations={mutations}");
            println!("queries={queries}");
            println!("errors={errors}");
            println!("keys_approx={keys_approx}");
            Ok(())
        }
        Response::Err { message } => Err(message),
    }
}
