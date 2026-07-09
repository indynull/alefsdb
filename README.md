# alefsdb

**alefsdb** is a local, research-oriented database of typed values arranged in a path hierarchy. The same data is available through a CLI (and library crates), a **Unix-socket daemon**, and a Linux **FUSE** mount. Search uses a small structured language, **AlefQL**.

It prioritizes clear layering and testable semantics over production features such as clustering or multi-tenant ops.

## Features

- **Typed values** at paths: scalars (`null`, `bool`, `int`, `float`, `string`, `bytes`) and structures (`hash`, `list`, `set`, `tree`)
- **Durable single-node storage**: write-ahead log with fsync on commit; optional compaction to bound WAL growth
- **Single-writer daemon** on a Unix socket (`<data>/alefs.sock`) so CLI and FUSE share one open store
- **Explicit namespace**: directories and values share one path tree; parents must exist before children (no auto-`mkdir`)
- **FUSE projection**: browse and edit with normal tools; type metadata via `user.alefs.type`
- **AlefQL**: path/type/structure predicates with `AND` / `OR` / `NOT`
- **Export / import** of the namespace as JSON
- **Load generator** (`alefs-bench`) for multi-client SET/GET mixes (memtier-style)

## Requirements

- Rust stable (edition 2021)
- Linux for FUSE mount (`libfuse3` headers to **build**; `fuse3` to mount)

```bash
# Debian / Ubuntu
sudo apt-get install -y libfuse3-dev pkg-config fuse3
```

## Build and test

```bash
cargo test --workspace
cargo build -p alefsdb -p alefs-bench
```

CI runs `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace`. FUSE mounts are optional in tests (skipped without `/dev/fuse`).

Soak helper (many writes + compact):

```bash
./scripts/load_compact.sh 500
```

## Quick start

### Direct mode (no daemon)

```bash
DATA=./data
alefsdb mkdir --data "$DATA" --direct /users
alefsdb set --data "$DATA" --direct /users/name --type string --value alice
alefsdb get --data "$DATA" --direct /users/name
```

### Daemon mode (recommended when using FUSE or many clients)

```bash
DATA=./data
mkdir -p "$DATA" /tmp/alefs-mnt

# Terminal 1: socket + optional FUSE on the same DbHandle
alefsdb serve --data "$DATA" --mount /tmp/alefs-mnt

# Terminal 2: CLI uses <data>/alefs.sock automatically
alefsdb mkdir --data "$DATA" /users
alefsdb set --data "$DATA" /users/name --type string --value alice
alefsdb query --data "$DATA" 'type string AND path /users/*'
alefsdb compact --data "$DATA"

# Filesystem view
ls /tmp/alefs-mnt/users
cat /tmp/alefs-mnt/users/name
```

Pass `--direct` on any command to open the store in-process and ignore the socket.

### Benchmark (memtier-style)

```bash
# Against a running daemon
alefsdb serve --data ./data &
alefs-bench --data ./data --clients 8 --requests 50000 --ratio 1:10 --keyspace 5000

# Or open-local (slower; re-opens store per op)
alefs-bench --data ./data --direct --clients 4 --requests 10000
```

Reports throughput (ops/sec) and latency percentiles (p50/p95/p99).

## Data model

### Paths

- Absolute only: `/`, `/a/b`
- No `.`, `..`, or empty segments in stored paths
- A path is either a **directory** or a **value**, never both
- Creating `/a/b` requires `/a` (or root) to already exist as a directory

### Values

| Kind | Meaning |
| --- | --- |
| Scalar | `null`, `bool`, `int`, `float`, `string`, `bytes` |
| Hash | String keys → nested values |
| List | Ordered sequence, indexable as `0`, `1`, … |
| Set | Unique members by **canonical encoding** |
| Tree | Ordered map with scalar keys |

Equality and set membership use a versioned **canonical binary encoding**. Floats compare by bit pattern.

### On-disk layout (`--data`)

| File | Role |
| --- | --- |
| `wal.log` | Append-only commit log |
| `checkpoint.bin` | Optional compacted snapshot |
| `alefs.sock` | Daemon Unix socket (while `serve` is running) |

Commits are durable after WAL `fdatasync`. A truncated final WAL record is dropped on open; prior commits remain.

## CLI reference

| Command | Purpose |
| --- | --- |
| `serve --data DIR [--mount PATH] [--socket PATH]` | Daemon: RPC socket (+ optional FUSE) |
| `mkdir / set / get / ls / rm` | Namespace basics |
| `hset` / `lpush` / `sadd` / `tset` | Structure helpers |
| `query '…'` | AlefQL (read-only) |
| `compact` | Checkpoint live state and truncate WAL |
| `export` / `import` | JSON dump / load |
| `--direct` | Skip socket; open store in this process |

## AlefQL

Queries return matching paths and type names. Empty results are success; syntax errors fail the process.

**Composition:** `NOT` binds tightest; `AND` and `OR` are left-associative with equal precedence.

| Predicate | Example |
| --- | --- |
| `path` | `path /users/**` |
| `type` | `type hash` (also `scalar`, `int`, …) |
| `name` | `name "*.tmp"` |
| `value` | `value = 3` |
| `has` | `has "email"` |
| `contains` | `contains "admin"` |
| `at` | `at 0 = "login"` |
| `key` | `key >= 10` |
| `size` | `size > 0` |

Wrong-type predicates do not match (they do not error).

## FUSE mount

| DB | Filesystem |
| --- | --- |
| Directory | Directory |
| Scalar | Regular file |
| Hash / list / set / tree | Directory of projected children |
| Type | `user.alefs.type` xattr |
| Set member display | `user.alefs.member` xattr |

Rename is `ENOTSUP` (blocks common editor temp-file workflows). Details: [docs/fuse-edit-paths.md](docs/fuse-edit-paths.md).

## Architecture

```
CLI / bench  ──┐
               ├── Unix socket ──► Daemon (single writer) ──► Namespace ──► Storage
FUSE mount  ───┘                        │
                                        └── same DbHandle
```

| Crate | Role |
| --- | --- |
| `alefs-types` | `Value`, canonical encode/decode, `DbPath` |
| `alefs-storage` | `Storage` trait; memory; WAL + compaction |
| `alefs-namespace` | Path graph, structure helpers, export/import |
| `alefs-query` | AlefQL parse + evaluate |
| `alefs-server` | Unix-socket RPC + dispatch |
| `alefs-fuse` | FUSE projection |
| `alefsdb` | CLI |
| `alefs-bench` | Multi-client load generator |

## Design and contributing

- Full design: [docs/superpowers/specs/2026-07-09-alefsdb-design.md](docs/superpowers/specs/2026-07-09-alefsdb-design.md)
- Working conventions: [AGENTS.md](AGENTS.md)

## License

MIT
