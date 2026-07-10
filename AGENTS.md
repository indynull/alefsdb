# AGENTS.md — alefsdb

Instructions for anyone (human or agent) working in this repository.

## What this is

**alefsdb** is a typed structure database with a Unix-socket daemon, FUSE mount, and AlefQL. Target: **production-grade single-node local store** — correctness, clear layering, operability. Not multi-node, not multi-tenant SaaS.

Canonical design: `docs/superpowers/specs/2026-07-09-alefsdb-design.md`.  
P6/P7 plan: `docs/superpowers/plans/2026-07-09-alefsdb-p6-p7.md`.

## Architecture (do not violate)

```
CLI / bench / FUSE  →  Unix socket  →  Daemon dispatch  →  Namespace  →  Storage  →  disk
```

- **One store:** FUSE and RPC share one `DbHandle`. Never write host files under `--data` from FUSE except via Storage.
- **Stable traits:** Prefer extending `Storage`, `Value`, `DbPath`, and RPC `Request`/`Response`.
- **Type-stable FUSE writes:** shell/editor writes must not silently change value types.
- **Explicit mkdir:** no auto-creating parent directories.
- **Single-writer mutations:** serialize writes through the daemon mutex; concurrent **connections** OK.
- **Data-dir flock:** only one `serve` process per data directory.

## Phases

| Phase | Scope | Status |
| --- | --- | --- |
| P0–P5 | Core DB, FUSE, AlefQL, compact, export | done |
| Post-P5 | Daemon, tests, soak, bench | done |
| **P6** | Transactions, indexes, persistent client | active |
| **P7** | Flock, concurrent accept, shutdown, stats, tracing, ops docs | active |

## Commits (mandatory)

**Never land multi-phase god commits.** One concern per commit; tests with behavior; message says why.

Suggested order: **plan → namespace/txn → indexes → server/ops → client → cli → bench → docs**.

## Engineering norms

1. TDD for behavior.
2. Small commits.
3. `cargo test` green before push; FUSE tests skip without `/dev/fuse`.
4. YAGNI on multi-node / Redis wire / full POSIX.
5. Lean CI: fmt, clippy `-D warnings`, test.
6. Push coherent slices.
7. Docs live with behavior.

## Crate map

| Crate | Owns |
| --- | --- |
| `alefs-types` | `Value`, codec, `DbPath` |
| `alefs-storage` | `Storage`, WAL, compaction, memory |
| `alefs-namespace` | path graph, structure ops, **transactions**, indexes |
| `alefs-query` | AlefQL parse + evaluate (uses indexes when present) |
| `alefs-server` | Unix RPC, dispatch, client, daemon lifecycle |
| `alefs-fuse` | FUSE adapter |
| `alefsdb` | CLI |
| `alefs-bench` | load generator |

## Tone

Be direct, prefer simple designs that match the spec, leave the tree cleaner than you found it.
