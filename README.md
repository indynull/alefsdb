# alefsdb

Research-oriented **typed structure database** that also mounts as a real **FUSE filesystem**, with a structured query language (**AlefQL**).

Design: [docs/superpowers/specs/2026-07-09-alefsdb-design.md](docs/superpowers/specs/2026-07-09-alefsdb-design.md)  
Agent guide: [AGENTS.md](AGENTS.md)  
FUSE edit paths: [docs/fuse-edit-paths.md](docs/fuse-edit-paths.md)

## Status

| Phase | Status |
| --- | --- |
| P0 Skeleton | done |
| P1 Durable KV (WAL + dirs/scalars + CLI) | done |
| P2 Structure types (hash/set/list/tree) | done |
| P3 FUSE mount | done |
| P4 AlefQL | done |
| P5 Compaction, crash tests, export/import | done |

## Build & test

```bash
# Debian/Ubuntu: headers needed to compile the FUSE crate
sudo apt-get install -y libfuse3-dev pkg-config fuse3

cargo test --workspace
cargo run -p alefsdb -- --help
```

CI runs `fmt`, `clippy -D warnings`, and `cargo test` (no FUSE mounts).

## Quick start

```bash
DATA=./data
mkdir -p "$DATA" /tmp/alefs-mnt

cargo run -p alefsdb -- mkdir --data "$DATA" /users
cargo run -p alefsdb -- set --data "$DATA" /users/name --type string --value alice
cargo run -p alefsdb -- get --data "$DATA" /users/name
cargo run -p alefsdb -- query --data "$DATA" 'type string AND path /users/*'

# FUSE (requires fuse group / permissions)
cargo run -p alefsdb -- serve --data "$DATA" --mount /tmp/alefs-mnt
# elsewhere: ls /tmp/alefs-mnt/users ; cat /tmp/alefs-mnt/users/name
```

## Crates

| Crate | Role |
| --- | --- |
| `alefs-types` | `Value`, canonical codec, `DbPath` |
| `alefs-storage` | `Storage` trait, memory (S0), WAL (S1), compact (S2) |
| `alefs-namespace` | Path graph, structure ops, export/import |
| `alefs-query` | AlefQL parse + evaluate |
| `alefs-fuse` | FUSE projection |
| `alefsdb` | CLI |

## License

MIT
