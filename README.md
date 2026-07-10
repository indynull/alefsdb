# alefsdb

**alefsdb** is a local, production-oriented **typed structure database** with:

- hierarchical paths and typed values (scalars, hash, list, set, tree)
- durable WAL storage with compaction
- a **Unix-socket daemon** (single writer, concurrent connections)
- optional **FUSE** mount sharing the same open store
- **AlefQL** search with type secondary indexes
- **atomic multi-op transactions**
- **alefs-bench** multi-client load generator

Not a distributed system: no replication, sharding, or multi-tenant ACLs.

## Requirements

- Rust stable (2021)
- Linux for FUSE (`libfuse3-dev`, `fuse3`)

```bash
sudo apt-get install -y libfuse3-dev pkg-config fuse3
cargo test --workspace
cargo build --release -p alefsdb -p alefs-bench
```

## Operations runbook

### Start the daemon

```bash
DATA=./data
mkdir -p "$DATA" /tmp/alefs-mnt
RUST_LOG=info alefsdb serve --data "$DATA" --mount /tmp/alefs-mnt
```

- Listens on `$DATA/alefs.sock`
- Takes an exclusive flock on `$DATA/LOCK` (second `serve` fails immediately)
- SIGINT / SIGTERM: stop accept loop, remove socket, release lock
- Optional FUSE mount uses the **same** `DbHandle` as RPC

### CLI (prefers daemon socket)

```bash
alefsdb mkdir --data "$DATA" /users
alefsdb set --data "$DATA" /users/name --type string --value alice
alefsdb get --data "$DATA" /users/name
alefsdb query --data "$DATA" 'type string AND path /users/*'
alefsdb stats --data "$DATA"
alefsdb compact --data "$DATA"
```

Use `--direct` to open the store in-process (ignores socket). Prefer the daemon under load or with FUSE.

### Atomic transactions

Write a JSON array of RPC ops to a file:

```json
[
  {"op": "mkdir", "path": "/acct"},
  {"op": "set", "path": "/acct/balance", "type": "int", "value": "100"},
  {"op": "hset", "path": "/acct/meta", "key": "currency", "type": "string", "value": "USD"}
]
```

```bash
alefsdb txn --data "$DATA" --file ops.json
```

All ops share one WAL commit; failure applies nothing.

### Benchmark

```bash
alefsdb serve --data ./data &
alefs-bench --data ./data --clients 8 --requests 50000 --ratio 1:10 --keyspace 5000
```

Workers keep a **persistent socket client**. Output includes ops/sec and p50/p95/p99 latency.

Soak (WAL growth + compact):

```bash
./scripts/load_compact.sh 500
```

## Data model

| Concept | Rules |
| --- | --- |
| Paths | Absolute `/a/b`; no `.` / `..` / empty segments; explicit mkdir |
| Values | scalar / hash / list / set / tree; equality via canonical encoding |
| On disk | `wal.log`, `checkpoint.bin`, `LOCK`, `alefs.sock` (while serving) |
| Indexes | `idx/t/<type>/<id>` for type-filtered queries |

## AlefQL

`NOT` tightest; `AND`/`OR` left-associative equal precedence. Wrong-type predicates → no match.

```text
path /users/** AND type hash AND has "email"
type set AND contains "admin" AND size > 0
```

Concrete `type` predicates use the secondary type index for candidate selection.

## FUSE

| DB | FS |
| --- | --- |
| Scalar | file (type-stable writes) |
| Structures | projected directories |
| Type | `user.alefs.type` |
| Set members | hash names + `user.alefs.member` |
| Rename | `ENOTSUP` |

See [docs/fuse-edit-paths.md](docs/fuse-edit-paths.md).

## Architecture

```
CLI / bench ──┐
              ├── Unix socket ──► Daemon (flock, stats, concurrent accept)
FUSE ─────────┘                        │
                                       ▼
                              Mutex<Database>  →  Namespace + indexes  →  WAL
```

| Crate | Role |
| --- | --- |
| `alefs-types` | values, codec, paths |
| `alefs-storage` | storage trait, WAL, compact |
| `alefs-namespace` | graph, txn, type index |
| `alefs-query` | AlefQL |
| `alefs-server` | RPC, daemon lifecycle, client |
| `alefs-fuse` | FUSE |
| `alefsdb` | CLI |
| `alefs-bench` | load generator |

## Design / plans

- [Design](docs/superpowers/specs/2026-07-09-alefsdb-design.md)
- [P6/P7 plan](docs/superpowers/plans/2026-07-09-alefsdb-p6-p7.md)
- [AGENTS.md](AGENTS.md)

## License

MIT
