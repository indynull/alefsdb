# alefsdb

**alefsdb** is a local, research-oriented database of typed values arranged in a path hierarchy. The same data is available through a CLI (and library crates) and as a real Linux **FUSE** mount. Search uses a small structured language, **AlefQL**.

It prioritizes clear layering and testable semantics over production features such as clustering or multi-tenant ops.

## Features

- **Typed values** at paths: scalars (`null`, `bool`, `int`, `float`, `string`, `bytes`) and structures (`hash`, `list`, `set`, `tree`)
- **Durable single-node storage**: write-ahead log with fsync on commit; optional compaction to bound WAL growth
- **Explicit namespace**: directories and values share one path tree; parents must exist before children (no auto-`mkdir`)
- **FUSE projection**: browse and edit with normal tools (`ls`, `cat`, redirects); type metadata via `user.alefs.type`
- **AlefQL**: path/type/structure predicates with `AND` / `OR` / `NOT`
- **Export / import** of the namespace as JSON (with type tags so directories and hashes stay distinct)

## Requirements

- Rust stable (edition 2021)
- Linux for FUSE mount (`libfuse3` headers to **build** the FUSE crate; `fuse3` to mount)

```bash
# Debian / Ubuntu
sudo apt-get install -y libfuse3-dev pkg-config fuse3
```

## Build and test

```bash
cargo test --workspace
cargo build -p alefsdb
./target/debug/alefsdb --help
```

CI runs `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace`. FUSE is compiled in CI; mounts are not exercised there.

## Quick start

All commands take `--data <dir>`: the on-disk store directory (created if needed).

```bash
DATA=./data

# Hierarchy: mkdir is explicit
alefsdb mkdir --data "$DATA" /users
alefsdb set --data "$DATA" /users/name --type string --value alice
alefsdb get --data "$DATA" /users/name
alefsdb ls --data "$DATA" /users

# Structures
alefsdb hset --data "$DATA" /users/profile --key email --type string --value a@b.c
alefsdb lpush --data "$DATA" /users/events --type string --value login
alefsdb sadd --data "$DATA" /users/roles --type string --value admin
alefsdb tset --data "$DATA" /users/scores --key 10 --type int --value 100

# Search
alefsdb query --data "$DATA" 'path /users/** AND type hash AND has "email"'

# Maintenance
alefsdb compact --data "$DATA"
alefsdb export --data "$DATA" --out snapshot.json
alefsdb import --data "$DATA" --file snapshot.json

# Filesystem view (blocking)
mkdir -p /tmp/alefs-mnt
alefsdb serve --data "$DATA" --mount /tmp/alefs-mnt
# another shell: ls /tmp/alefs-mnt/users && cat /tmp/alefs-mnt/users/name
```

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
| Hash | String keys → nested values (unordered map; encoding sorts keys) |
| List | Ordered sequence, indexable as `0`, `1`, … |
| Set | Unique members by **canonical encoding** |
| Tree | Ordered map with scalar keys |

Equality and set membership use a versioned **canonical binary encoding** (not JSON). Floats compare by bit pattern.

### On-disk layout (`--data`)

| File | Role |
| --- | --- |
| `wal.log` | Append-only commit log (S1) |
| `checkpoint.bin` | Optional compacted snapshot of live key-value state (S2) |

Namespace nodes and children are stored as keys inside that engine (`node/…`, `child/…`, `meta/…`). Commits are durable after WAL `fdatasync`. A truncated final WAL record (crash mid-write) is dropped on open; prior commits remain.

## CLI reference

| Command | Purpose |
| --- | --- |
| `mkdir --data DIR PATH` | Create directory (parent must exist) |
| `set --data DIR PATH --type T --value V` | Create/replace a value (`string`, `int`, `bool`, `float`, `null`, `bytes`, or empty `hash`/`list`/`set`/`tree`) |
| `get --data DIR PATH` | Show type and value summary |
| `ls --data DIR [PATH]` | List children (`dir` / `val`) |
| `rm --data DIR PATH` | Delete empty directory or value |
| `hset` / `lpush` / `sadd` / `tset` | Mutate hash / list / set / tree at a path |
| `query --data DIR '…'` | Run AlefQL (read-only) |
| `compact --data DIR` | Checkpoint live state and truncate WAL |
| `export --data DIR [--out FILE]` | Dump namespace JSON |
| `import --data DIR --file FILE` | Load JSON into the store |
| `serve --data DIR --mount PATH` | Mount FUSE (blocks until unmount) |

Each invocation opens the store, runs the operation, and exits (except `serve`). There is no separate long-lived control daemon for CLI commands.

## AlefQL

Queries return matching paths and type names. Empty results are success; syntax errors fail the process.

**Composition:** `NOT` binds tightest; `AND` and `OR` are left-associative with equal precedence. Use parentheses when mixing them.

**Predicates (sketch):**

| Predicate | Example | Notes |
| --- | --- | --- |
| `path` | `path /users/**` | Glob; `*` one segment, `**` any depth |
| `type` | `type hash` | Also `scalar`, `int`, `string`, `list`, … |
| `name` | `name "*.tmp"` | Last path segment glob |
| `value` | `value = 3` | Scalars only |
| `has` | `has "email"` | Hash / tree keys |
| `contains` | `contains "admin"` | Set, list, or structure values |
| `at` | `at 0 = "login"` | List index + scalar compare |
| `key` | `key >= 10` | Tree keys |
| `size` | `size > 0` | Structure length / map size |

Wrong-type predicates do not match (they do not error).

```text
path /users/** AND type hash AND has "email"
type set AND contains "admin" AND size > 0
NOT type scalar AND name "*.tmp"
```

## FUSE mount

`alefsdb serve` exposes the namespace under the mount point:

| DB | Filesystem |
| --- | --- |
| Directory | Directory |
| Scalar | Regular file (text for most scalars; raw bytes for `bytes`) |
| Hash / list / set / tree | Directory of projected children |
| Type | Extended attribute `user.alefs.type` |

**Supported edits** (summary): type-stable writes to scalar files (same scalar variant), `mkdir` / `rm` for namespace entries, hash key create/unlink and scalar field writes. Editors that rewrite via rename/temp files or change types are not fully supported.

Details: [docs/fuse-edit-paths.md](docs/fuse-edit-paths.md).

## Architecture

```
CLI / FUSE / Query  →  Namespace  →  Value/codec  →  Storage  →  disk
```

| Crate | Role |
| --- | --- |
| `alefs-types` | `Value`, canonical encode/decode, `DbPath` |
| `alefs-storage` | `Storage` trait; in-memory backend; WAL + compaction |
| `alefs-namespace` | Path graph, structure helpers, export/import |
| `alefs-query` | AlefQL parse + evaluate |
| `alefs-fuse` | FUSE adapter (projection only—no direct writes under `--data`) |
| `alefsdb` | CLI binary |

FUSE never writes host files inside `--data` except through the storage layer.

## Design and contributing

- Full design: [docs/superpowers/specs/2026-07-09-alefsdb-design.md](docs/superpowers/specs/2026-07-09-alefsdb-design.md)
- Working conventions for agents/humans: [AGENTS.md](AGENTS.md)

## License

MIT
