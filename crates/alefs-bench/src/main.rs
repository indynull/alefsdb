//! memtier-inspired load generator for alefsdb (Unix socket or direct open).

use alefs_server::{default_socket_path, dispatch, open_db, Client, Request, Response};
use clap::Parser;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "alefs-bench",
    about = "Load generator for alefsdb (SET/GET mix, multi-client)"
)]
struct Args {
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    direct: bool,
    #[arg(long, default_value_t = 4)]
    clients: usize,
    #[arg(long, default_value_t = 10_000)]
    requests: u64,
    #[arg(long, default_value = "1:10")]
    ratio: String,
    #[arg(long, default_value_t = 1000)]
    keyspace: u64,
    #[arg(long, default_value = "/bench")]
    prefix: String,
    #[arg(long, default_value_t = true)]
    warmup: bool,
}

#[derive(Clone, Copy)]
struct Ratio {
    set: u64,
    get: u64,
}

fn parse_ratio(s: &str) -> Result<Ratio, String> {
    let (a, b) = s
        .split_once(':')
        .ok_or_else(|| "ratio must be set:get".to_string())?;
    Ok(Ratio {
        set: a.parse().map_err(|e| format!("set side: {e}"))?,
        get: b.parse().map_err(|e| format!("get side: {e}"))?,
    })
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let ratio = parse_ratio(&args.ratio)?;
    if ratio.set + ratio.get == 0 {
        return Err("ratio sum must be > 0".into());
    }
    if args.clients == 0 {
        return Err("clients must be > 0".into());
    }

    let sock = default_socket_path(&args.data);
    let use_socket = !args.direct && sock.exists();
    if use_socket {
        eprintln!("mode=socket path={}", sock.display());
    } else {
        eprintln!("mode=direct data={}", args.data.display());
    }

    if args.warmup {
        one_call(
            &args.data,
            use_socket,
            Request::Mkdir {
                path: args.prefix.clone(),
            },
        )?;
        for i in 0..args.keyspace.min(16) {
            let _ = one_call(
                &args.data,
                use_socket,
                Request::Set {
                    path: format!("{}/k{i}", args.prefix),
                    type_name: "string".into(),
                    value: format!("warm{i}"),
                },
            );
        }
    }

    let per_client = args.requests / args.clients as u64;
    let remainder = args.requests % args.clients as u64;
    let errors = Arc::new(AtomicU64::new(0));
    let sets = Arc::new(AtomicU64::new(0));
    let gets = Arc::new(AtomicU64::new(0));

    let start = Instant::now();
    let mut handles = Vec::new();
    for c in 0..args.clients {
        let n = per_client + if (c as u64) < remainder { 1 } else { 0 };
        let data = args.data.clone();
        let prefix = args.prefix.clone();
        let errors = Arc::clone(&errors);
        let sets = Arc::clone(&sets);
        let gets = Arc::clone(&gets);
        let keyspace = args.keyspace.max(1);
        let sock_path = sock.clone();
        handles.push(thread::spawn(move || {
            let mut latencies = Vec::with_capacity(n as usize);
            // Persistent client when on socket.
            let mut client = if use_socket {
                Some(Client::connect(&sock_path).ok())
            } else {
                None
            };
            for seq in 0..n {
                let key_id = (c as u64 * 1_000_000 + seq) % keyspace;
                let path = format!("{prefix}/k{key_id}");
                let do_set = is_set(seq, ratio);
                let req = if do_set {
                    sets.fetch_add(1, Ordering::Relaxed);
                    Request::Set {
                        path,
                        type_name: "string".into(),
                        value: format!("v{seq}"),
                    }
                } else {
                    gets.fetch_add(1, Ordering::Relaxed);
                    Request::Get { path }
                };
                let t0 = Instant::now();
                let result = if let Some(Some(ref mut cl)) = client.as_mut() {
                    cl.call(req.clone()).map_err(|e| e.to_string())
                } else if use_socket {
                    // reconnect
                    match Client::connect(&sock_path) {
                        Ok(mut cl) => {
                            let r = cl.call(req).map_err(|e| e.to_string());
                            client = Some(Some(cl));
                            r
                        }
                        Err(e) => Err(e.to_string()),
                    }
                } else {
                    match open_db(&data) {
                        Ok(db) => Ok(dispatch(&db, req)),
                        Err(e) => Err(e.to_string()),
                    }
                };
                match result {
                    Ok(Response::Ok { .. }) | Ok(Response::Value { .. }) => {
                        latencies.push(t0.elapsed());
                    }
                    Ok(Response::Err { .. }) if !do_set => {
                        latencies.push(t0.elapsed());
                    }
                    Ok(Response::Err { .. }) | Err(_) => {
                        if do_set {
                            errors.fetch_add(1, Ordering::Relaxed);
                        } else {
                            latencies.push(t0.elapsed());
                        }
                    }
                    Ok(_) => latencies.push(t0.elapsed()),
                }
            }
            latencies
        }));
    }

    let mut all_lat = Vec::new();
    for h in handles {
        match h.join() {
            Ok(lat) => all_lat.extend(lat),
            Err(_) => {
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let elapsed = start.elapsed();
    let total = sets.load(Ordering::Relaxed) + gets.load(Ordering::Relaxed);
    let err_n = errors.load(Ordering::Relaxed);
    let ops = all_lat.len() as u64;
    let secs = elapsed.as_secs_f64().max(1e-9);
    let rps = ops as f64 / secs;

    all_lat.sort();
    let p50 = percentile(&all_lat, 50);
    let p95 = percentile(&all_lat, 95);
    let p99 = percentile(&all_lat, 99);

    println!("=== alefs-bench ===");
    println!("clients={}", args.clients);
    println!("requests_planned={}", args.requests);
    println!("completed_ops={ops}");
    println!("sets={}", sets.load(Ordering::Relaxed));
    println!("gets={}", gets.load(Ordering::Relaxed));
    println!("errors={err_n}");
    println!("duration_sec={secs:.3}");
    println!("throughput_ops_sec={rps:.1}");
    println!("latency_p50_us={}", p50.as_micros());
    println!("latency_p95_us={}", p95.as_micros());
    println!("latency_p99_us={}", p99.as_micros());
    if total > 0 {
        println!(
            "mix_set_pct={:.1}",
            100.0 * sets.load(Ordering::Relaxed) as f64 / total as f64
        );
    }
    Ok(())
}

fn is_set(seq: u64, ratio: Ratio) -> bool {
    let cycle = ratio.set + ratio.get;
    let pos = seq % cycle;
    pos < ratio.set
}

fn percentile(sorted: &[Duration], p: u32) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((p as usize) * (sorted.len() - 1)) / 100;
    sorted[idx]
}

fn one_call(data: &PathBuf, use_socket: bool, req: Request) -> Result<Response, String> {
    if use_socket {
        let mut c = Client::connect(default_socket_path(data)).map_err(|e| e.to_string())?;
        c.call(req).map_err(|e| e.to_string())
    } else {
        let db = open_db(data).map_err(|e| e.to_string())?;
        Ok(dispatch(&db, req))
    }
}
