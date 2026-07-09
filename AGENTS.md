# AGENTS.md — alefsdb

Instructions for anyone (human or agent) working in this repository.

## What this is

**alefsdb** is a research-oriented typed structure database that also mounts as a FUSE filesystem, with AlefQL search. Optimize for **correctness, clear layering, and testable semantics** — not production multi-tenant ops or premature performance.

Canonical design: `docs/superpowers/specs/2026-07-09-alefsdb-design.md`.

## Architecture (do not violate)

```
CLI / FUSE / Query  →  Namespace  →  Value/codec  →  Storage trait  →  disk
```

- **One store:** FUSE is a projection of Namespace ops. Never write host files under `--data` from the FUSE adapter except via Storage.
- **Stable traits:** Prefer extending `Storage`, `Value`, `DbPath` over ad-hoc shortcuts.
- **Type-stable FUSE writes:** shell/editor writes must not silently change value types.
- **Explicit mkdir:** no auto-creating parent directories.
- **Single-writer v1:** serialize mutations; correctness over parallel write throughput.

## Phases

| Phase | Scope |
| --- | --- |
| P0 | Workspace, types, codec, paths, Storage trait |
| P1 | WAL storage, namespace, scalar/dir CLI |
| P2 | hash / set / list / tree ops |
| P3 | FUSE mount |
| P4 | AlefQL |
| P5 | Compaction, crash tests, export/import, docs |

Do not skip layering to “finish” a later phase.

## Commits (mandatory)

**Never land a multi-phase “god commit.”** Prefer several small commits on the same branch over one large one.

| Rule | Detail |
| --- | --- |
| One concern per commit | e.g. “WAL storage”, not “WAL + FUSE + query + CI” |
| Message = why | Imperative subject; body explains motivation if non-obvious |
| Buildable steps | Each commit should leave the workspace compiling for crates it touches (`cargo test -p …` or workspace as appropriate) |
| Tests with behavior | Behavior change and its tests land together in the same commit |
| History rewrite | If you already pushed a god commit, split it (soft reset / recommit) and force-push only when the user expects that branch to move |

Suggested slice order for stack work: **process/docs → storage → namespace → query → fuse → cli → user docs**.

Bad: `Implement P1–P5: everything`  
Good: `Add S1 WAL storage with truncated-tail recovery` then `Add namespace graph for dirs and values` then …

## Engineering norms

1. **TDD for behavior** — failing test first for new behavior and bugfixes.
2. **Small commits** — see above; no phase-spanning dumps.
3. **`cargo test` green** before push. FUSE integration tests may be ignored when `/dev/fuse` is unavailable.
4. **YAGNI** — no multi-node, Redis wire protocol, or full POSIX.
5. **CI is lean** — one workflow: `fmt --check`, `clippy -D warnings`, `cargo test --workspace`. No matrix sprawl, no required FUSE mounts in CI, no deploy noise.
6. **Push when a coherent slice is ready** so reviewers can see progress outside the container—not only at end of a multi-hour marathon.
7. **Docs live with behavior** — update design/plan/README when semantics change; document supported FUSE edit paths when touching P3+.

## Crate map

| Crate | Owns |
| --- | --- |
| `alefs-types` | `Value`, codec, `DbPath` |
| `alefs-storage` | `Storage`, WAL (S1), compaction (S2), memory (S0) |
| `alefs-namespace` | path → nodes, structure ops |
| `alefs-query` | AlefQL parse + evaluate |
| `alefs-fuse` | FUSE adapter (optional feature / Linux) |
| `alefsdb` (cli) | user commands |

## Tone

Be direct, prefer simple designs that match the spec, and leave the tree cleaner than you found it. When unsure, re-read the design doc and these rules before inventing APIs.
