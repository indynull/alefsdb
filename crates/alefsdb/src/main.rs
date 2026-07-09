use alefs_server::{
    default_socket_path, dispatch, open_db, rpc_call, serve_listener, Request, Response,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

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
            let db = open_db(&data).map_err(|e| e.to_string())?;
            if let Some(mnt) = mount {
                let db_fuse = Arc::clone(&db);
                let mnt_c = mnt.clone();
                thread::spawn(move || {
                    if let Err(e) = alefs_fuse::mount_shared(db_fuse, &mnt_c) {
                        eprintln!("fuse error: {e}");
                    }
                });
                eprintln!("fuse mounting at {}", mnt.display());
            }
            serve_listener(db, sock).map_err(|e| e.to_string())?;
            Ok(())
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
        Response::Err { message } => Err(message),
    }
}
