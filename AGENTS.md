# AGENTS.md — alefsdb

Instructions for anyone (human or agent) working in this repository.

## What this is

**alefsdb** is a research-oriented typed structure database that also mounts as a FUSE filesystem, with AlefQL search. Optimize for **correctness, clear layering, and testable semantics** — not production multi-tenant ops or premature performance.

Canonical design: `docs/superpowers/specs/2026-07-09-alefsdb-design.md`.

## Architecture (do not violate)

```
CLI / bench / FUSE  →  (Unix socket)  →  Daemon dispatch  →  Namespace  →  Storage  →  disk
```

- **One store:** FUSE and RPC share one `DbHandle`. Never write host files under `--data` from FUSE except via Storage.
- **Stable traits:** Prefer extending `Storage`, `Value`, `DbPath`, and the RPC `Request`/`Response` types over ad-hoc shortcuts.
- **Type-stable FUSE writes:** shell/editor writes must not silently change value types.
- **Explicit mkdir:** no auto-creating parent directories.
- **Single-writer v1:** serialize mutations through the daemon mutex; correctness over parallel write throughput.

## Phases

| Phase | Scope |
| --- | --- |
| P0–P5 | Types, WAL, namespace, structures, FUSE, AlefQL, compact, export — **landed** |
| Post-P5 | Daemon socket, FUSE tests, AlefQL golden matrix, soak, bench |
| P6+ | Secondary indexes, multi-op transaction UX, richer tooling (as needed) |

Do not skip layering to “finish” a later phase.

## Commits (mandatory)

**Never land a multi-phase “god commit.”** Prefer several small commits on the same branch over one large one.

| Rule | Detail |
| --- | --- |
| One concern per commit | e.g. “WAL storage”, not “WAL + FUSE + query + CI” |
| Message = why | Imperative subject; body explains motivation if non-obvious |
| Buildable steps | Each commit should leave the workspace compiling for crates it touches |
| Tests with behavior | Behavior change and its tests land together in the same commit |
| History rewrite | Split god commits when needed; force-push only when expected |

Suggested slice order: **process/docs → storage → namespace → query → server/rpc → fuse → cli → bench → user docs**.

Bad: `Implement everything remaining`  
Good: `Add Unix-socket RPC server crate` then `Wire CLI to daemon socket` then …

## Engineering norms

1. **TDD for behavior** — failing test first for new behavior and bugfixes.
2. **Small commits** — see above; no phase-spanning dumps.
3. **`cargo test` green** before push. FUSE integration tests skip when `/dev/fuse` is unavailable.
4. **YAGNI** — no multi-node, Redis wire protocol, or full POSIX.
5. **CI is lean** — one workflow: `fmt --check`, `clippy -D warnings`, `cargo test --workspace`. No matrix sprawl, no required FUSE mounts in CI.
6. **Push when a coherent slice is ready** so reviewers can see progress outside the container.
7. **Docs live with behavior** — update README when user-facing semantics change; keep fuse-edit-paths accurate.

## Crate map

| Crate | Owns |
| --- | --- |
| `alefs-types` | `Value`, codec, `DbPath` |
| `alefs-storage` | `Storage`, WAL (S1), compaction (S2), memory (S0) |
| `alefs-namespace` | path → nodes, structure ops |
| `alefs-query` | AlefQL parse + evaluate |
| `alefs-server` | Unix-socket protocol, dispatch, client helper |
| `alefs-fuse` | FUSE adapter (shares `DbHandle` with daemon) |
| `alefsdb` | user CLI |
| `alefs-bench` | multi-client SET/GET load generator |

## Tone

Be direct, prefer simple designs that match the spec, and leave the tree cleaner than you found it. When unsure, re-read the design doc and these rules before inventing APIs.
