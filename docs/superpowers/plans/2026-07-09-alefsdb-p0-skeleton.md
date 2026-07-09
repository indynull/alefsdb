# alefsdb P0 Skeleton — Implementation Plan

> **For agentic workers:** Execute task-by-task with TDD. Steps use checkbox syntax.

**Goal:** Stand up the Rust workspace with `Value` algebra, canonical encode/decode, path types, and a `Storage` trait so later phases have stable foundations.

**Architecture:** Multi-crate Cargo workspace. `alefs-types` owns values + paths + codec. `alefs-storage` owns the durable-store trait (and a test-only memory backend). No FUSE, CLI, or WAL yet.

**Tech Stack:** Rust 2021, Cargo workspace, std only (no external deps in P0).

**Spec:** `docs/superpowers/specs/2026-07-09-alefsdb-design.md` §4, §5.1, §11 P0, PR1–PR3 (trait only).

---

## File map

| Path | Responsibility |
| --- | --- |
| `Cargo.toml` | Workspace members |
| `README.md` | Project blurb + how to test |
| `crates/alefs-types/Cargo.toml` | types crate |
| `crates/alefs-types/src/lib.rs` | module exports |
| `crates/alefs-types/src/value.rs` | `Scalar`, `Value` |
| `crates/alefs-types/src/codec.rs` | canonical encode/decode |
| `crates/alefs-types/src/path.rs` | absolute path parse/validate |
| `crates/alefs-storage/Cargo.toml` | storage crate |
| `crates/alefs-storage/src/lib.rs` | `Storage` trait, `WriteBatch`, `MemoryStorage` (S0) |

---

### Task 1: Workspace skeleton

**Files:** Create `Cargo.toml`, `README.md`, crate manifests, empty `lib.rs` files.

- [x] Create workspace and empty crates that `cargo test` can run

### Task 2: Value types + codec (TDD)

**Files:** `value.rs`, `codec.rs`

- [x] Tests: round-trip scalars, structures, set equality via encoding, depth
- [x] Implement `Value` / `Scalar` and versioned canonical binary codec

### Task 3: Paths (TDD)

**Files:** `path.rs`

- [x] Tests: accept `/`, `/a/b`; reject ``, `a/b`, `/./x`, `/a//b`, `/a/../b`
- [x] Implement `DbPath`

### Task 4: Storage trait + memory backend

**Files:** `alefs-storage`

- [x] Trait matching design (`get`, batch put/delete, `commit`, `scan_prefix`)
- [x] `MemoryStorage` for tests; basic put/get/delete/scan tests

### Task 5: Verify and publish

- [x] `cargo test` clean
- [x] Commit and push to origin
