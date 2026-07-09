# alefsdb — Design Document

**Date:** 2026-07-09  
**Status:** Ready for implementation planning (spec review)  
**Codename:** alefsdb

A research-oriented store that is both a typed structure database and a real filesystem.

---

## 1. Context and product decisions

### 1.1 Purpose

**Research / learning platform.** Optimize for correctness, clear layering, and explorable semantics—not production multi-tenant operations, clustering, or peak throughput.

### 1.2 Locked decisions

| Topic | Decision |
| --- | --- |
| Purpose | Research / learning; correctness and clarity first |
| Filesystem surface | Real OS mount via **FUSE** (first-class, not deferred indefinitely) |
| Durability | **Persistent on disk** from the first real `serve` path |
| Type system | Full suite from day one in the algebra: scalar, hash, set, list/array, tree/ordered map |
| Search | **Structured query language** with per-type operators and boolean composition |
| Architecture | **Layered typed engine + FUSE adapter** (Approach 1) |
| Language | **Rust** (2021 edition) |
| FUSE stack | `fuser` crate; **Linux first** |
| Storage evolution | Stable `Storage` trait; S1 = WAL + in-memory index; later compaction / optional engine swap |

### 1.3 Approaches considered

1. **Layered typed engine + FUSE adapter (chosen)** — Single daemon; FUSE and query are projections over one namespace and type layer; storage is swappable behind a trait. Best dual DB/FS identity for research.
2. **Host filesystem as source of truth** — Encode structures as host dirs/files. Fast demos, weak typed DB semantics, poor atomicity and query story.
3. **Embed existing store + overlay** — RocksDB/SQLite/sled under a typed namespace. Persistence “free,” but end-to-end research and type-aware layout are constrained by the embed.

**Pragmatic note inside Approach 1:** S1 storage is intentionally simple (WAL + memory index). A production LSM is optional later and must not rewrite upper layers.

---

## 2. Goals, non-goals, and requirements

### 2.1 Goals

1. **Dual interface:** The same authoritative data is reachable via a programmatic/typed API and a **real FUSE mount** (`ls`, `cat`, editors, scripts).
2. **Typed structures:** First-class values—**scalar**, **hash**, **set**, **list/array**, **tree/ordered map**—present in the type system from the start even if implementation is phased.
3. **Persistence:** Single-node, on-disk durability; process restart preserves committed state.
4. **Structured search:** A small query language with per-type operators and boolean composition (`AND` / `OR` / `NOT`).
5. **Research quality:** Prefer clear invariants, explicit layers, and testable semantics over clustering or polish.

### 2.2 Non-goals

- Multi-node replication, sharding, consensus
- Full POSIX compliance (no devices, sockets, exec; permissions stay a simple local policy)
- Wire-compatible Redis or SQL protocols
- Horizontal scale or multi-GB–TB performance as success criteria
- Rich multi-user auth / ACLs (optional later; default is single-trust local use)

### 2.3 High-level requirements

| ID | Requirement |
| --- | --- |
| R1 | Values have an explicit type; type is preserved across restart and visible in the FS projection |
| R2 | Hierarchical **namespace** maps 1:1 to a path tree used by both API and FUSE |
| R3 | Mutations are atomic at a defined granularity (at least single-key / single-structure op); durable after commit |
| R4 | FUSE reads/writes are projections of typed ops—not a second storage |
| R5 | Query language can select by path patterns and structure-aware predicates, with boolean composition |
| R6 | Layers communicate only through stable interfaces (Storage, Namespace, Value, Query, FsAdapter) |
| R7 | Invariants are enforceable in tests (type rules, path rules, durability after simulated crash where feasible) |

### 2.4 Success criteria

- Mount a DB, create typed structures via CLI/API, see and edit them with normal FS tools along **documented** edit paths
- Restart the process; data and types remain
- Run queries using path matchers and several type-specific operators with `AND` / `OR` / `NOT`
- A newcomer can read this document and point to where type, path, storage, FUSE, and query live

---

## 3. Architecture

### 3.1 Process shape

A single local **daemon** owns the open database and FUSE session:

- `alefsdb serve --data <dir> --mount <path>` — open store, mount FS, accept local control (Unix socket or localhost admin)
- `alefsdb` CLI — create/get/set/query against a running daemon (or open the store single-process for non-mounted tools)
- Library crates for the same layers so unit tests do not require FUSE

### 3.2 Layer stack

```
FsAdapter (FUSE)     QueryEngine      Control/CLI API
        \                 |                 /
         \                |                /
          v               v               v
              Namespace (paths + entries)
                        |
                        v
              Type / Value layer
                        |
                        v
              Storage trait  ←── S1 log+index first; swappable later
                        |
                        v
              On-disk files under --data
```

**Rule:** FUSE never writes host files under `--data` itself. It only calls Namespace / Value / Storage. The mount is a view, not a second store.

### 3.3 Component boundaries (Rust workspace)

| Component | Responsibility | Depends on |
| --- | --- | --- |
| `alefs-types` | Value algebra, canonical encode/decode, type errors | — |
| `alefs-storage` | `Storage` trait + S1 WAL/index impl | — |
| `alefs-namespace` | Path rules, dir/value nodes, tree ops | types, storage |
| `alefs-query` | AlefQL parse → AST → evaluate | namespace, types |
| `alefs-fuse` | FUSE adapter (projection + write-back rules) | namespace, types |
| `alefs-server` | Daemon lifecycle: open DB, mount, admin socket | all above |
| `alefs-cli` | User CLI: serve, get/set/mkdir, query | server protocol / lib |

### 3.4 Concurrency (v1)

- Single-process daemon
- **One writer** critical section: mutations serialized
- Concurrent readers allowed for API + FUSE getattr/read when it does not complicate S1
- Correctness over parallel write throughput

---

## 4. Path and type model

### 4.1 Paths

Paths are absolute, `/`-separated, UTF-8 segment names:

- No empty segments
- No `.` or `..` in **stored** paths
- Root `/` is always a directory

| Path concept | Meaning |
| --- | --- |
| **Directory entry** | Namespace node that holds children |
| **Value entry** | Namespace node bound to a typed value |
| **Root** | `/` |

A path addresses either a directory or a value, not both. Creating a value at `/a/b` requires intermediate directories to exist (**explicit mkdir** for research clarity—no silent auto-create of parents).

### 4.2 Value algebra

```text
Value =
  | Scalar(Null | Bool | Int | Float | String | Bytes)
  | Hash(Map<String, Value>)      // unordered string keys → nested values
  | Set(Set<Value>)               // uniqueness by canonical encoding
  | List(Vec<Value>)              // ordered, indexable
  | Tree(OrderedMap<Key, Value>)  // ordered keys; Key ⊆ Scalar
```

- Structures may nest values of any type.
- Configurable soft max depth for safety (default recommendation: 64).
- **Equality / set membership** use a **canonical encoding** (deterministic serialization).
- **Floats:** equality is by canonical byte encoding of the bit pattern (including NaN payload rules fixed in the codec)—not IEEE “numeric” equality. Document this for query and sets.

### 4.3 Filesystem projection

| DB concept | FS presentation (v1) |
| --- | --- |
| Directory entry | Directory |
| Scalar | Regular file (content = documented encoding of the scalar) |
| Hash | Directory of keys; each child projected by its value type |
| Set | Directory of members; names derived from a safe encoding / hash of canonical form |
| List | Directory with numeric names `0`, `1`, … (optional virtual `length` file) |
| Tree | Directory of keys with ordered `readdir` |

**Type metadata:** Extended attribute `user.alefs.type=<typename>` (and documented content encoding). Virtual control namespaces are deferred if xattrs suffice.

**Writes from FUSE (v1 policy):**

- Write to scalar file → replace scalar (**type-stable**; type changes only via explicit API/CLI)
- Create file/dir under hash → insert key (rules documented per type)
- Unlink → delete member/key
- List reorder/rename: documented subset only; unsupported editor patterns return `ENOTSUP`/`EPERM`
- Supported edit paths are listed in user docs and covered by integration tests

### 4.4 Invariants

1. Every value entry has exactly one type.
2. Path ↔ node id mapping is unique.
3. FUSE `readdir` of a structure matches Value-layer enumeration for the same committed state.
4. After `commit` + reopen, graph and types are identical.
5. Query results depend only on committed state (FUSE page-cache / flush policy documented so tools do not observe silent lies across fsync).

---

## 5. Storage

### 5.1 Storage trait

```text
get(key) -> Option<bytes>
put(key, bytes)      // into a write batch
delete(key)
scan(prefix) -> iterator
commit(batch) -> durable
```

Namespace and values are encoded into storage keys (for example `meta/…`, `node/<id>`, `child/<parent>/<name>`). Exact layout is an implementation detail behind the trait.

### 5.2 Storage phases

| Stage | Backend | Outcome |
| --- | --- | --- |
| **S0** | In-memory (+ optional snapshot) | Unit tests only—not default `serve` |
| **S1** | **Write-ahead log + in-memory primary index**; periodic checkpoint of index/manifest | Durable commits; simple crash recovery |
| **S2** | Compaction / segment GC for the log | Bounded disk growth |
| **S3** | Optional page-oriented or LSM engine behind the same trait | Performance research without rewriting upper layers |

**S1 commit definition:** A batch is durable after WAL `fsync` / `fdatasync` of the record that closes the batch. Checkpoint is an optimization, not required for correctness.

**On-disk value encoding:** Length-prefixed canonical binary with a version byte. JSON export/import is a tool, not the primary store format.

### 5.3 Crash and consistency bar

- Kill after commit → data present on reopen
- Kill mid-batch → that batch absent; prior commits intact
- FUSE: `fsync` on a scalar maps to commit of that write; flush policy documented
- No distributed consistency story

---

## 6. Query language (AlefQL)

### 6.1 Surface

- CLI: `alefsdb query '...'`
- Library AST: CLI parses into the same AST used by tests
- **v1 queries are read-only** (no update-from-query)

**Result model:** List of hits `{ path, type, optional preview }`. Full values only with an explicit flag (for example `--values`).

### 6.2 Syntax (v1)

```text
query     := expr
expr      := term ( ("AND" | "OR") term )*
term      := [ "NOT" ] primary
primary   := predicate | "(" expr ")"

predicate :=
    path PATH_GLOB
  | type TYPE_NAME
  | name STRING_GLOB
  | value SCALAR_CMP
  | has KEY                 // hash / tree
  | contains VALUE          // set / list / hash-values (per-type rules)
  | at INDEX CMP            // list
  | key KEY_CMP             // tree
  | size CMP
  | member SCALAR_CMP       // optional later: set/list scalar members
```

Comparisons: `=`, `!=`, `<`, `<=`, `>`, `>=`. Strings: **glob only** in v1 (consistent glob syntax in both `path`/`name` and string `value` matches).

**Boolean precedence:** `NOT` binds tightest; `AND` and `OR` are **left-associative with equal precedence**. Use parentheses for mixed `AND`/`OR` clarity. (No SQL-style “AND tighter than OR” unless we deliberately change this later.)

**PATH_GLOB examples:** `/users/**`, `/config/*`, exact `/a/b`.

**Examples:**

```text
path /users/** AND type hash AND has "email"
type set AND contains "admin" AND size > 0
path /events/* AND type list AND at 0 = "login"
type tree AND key >= 100 AND key < 200
NOT type scalar AND name "*.tmp"
```

### 6.3 Operator applicability

| Operator | Scalar | Hash | Set | List | Tree |
| --- | --- | --- | --- | --- | --- |
| `path` / `name` / `type` / `size` | yes | yes | yes | yes | yes |
| `value` | yes | — | — | — | — |
| `has` | — | yes | — | — | yes |
| `contains` | — | values | yes | yes | values |
| `at` | — | — | — | yes | — |
| `key` | — | — | — | — | yes |

**Semantics:** Applying an operator to a non-applicable type yields **no match** for that node (not a hard error). Parse errors are distinct from empty results in CLI exit codes.

### 6.4 Evaluation strategy (v1)

1. If `path` is present, resolve candidates via path prefix / glob walk; else full namespace walk (acceptable at research scale).
2. Filter by `type`, `name`, then structure predicates via Value-layer reads.
3. Boolean composition as AST evaluation.
4. No cost-based planner; secondary indexes are a later phase without changing language surface.

---

## 7. Data flows

### 7.1 API/CLI write

```text
CLI → parse → Namespace.mutate → Value encode → Storage.batch + commit
                                             → optional FUSE invalidate
```

### 7.2 FUSE read

```text
kernel → FsAdapter.lookup/read/readdir → Namespace resolve → project attrs/content
```

### 7.3 FUSE write (scalar)

```text
kernel → write/flush/fsync → decode as scalar (type-stable)
      → Namespace.replace_value → Storage commit
```

### 7.4 Query

```text
CLI → AlefQL parse → QueryEngine → candidates → predicates → hits
```

### 7.5 Restart

```text
serve → Storage.recover(WAL + checkpoint) → Namespace open → mount FUSE
```

---

## 8. Error model

| Class | Examples | Surface |
| --- | --- | --- |
| User/input | Bad path, type mismatch on write, query parse error | CLI stderr + exit code; FUSE errno (`EINVAL`, `EISDIR`, `ENOTDIR`, `ENOENT`, …) |
| Policy | Type change via FUSE forbidden; unsupported rename | FUSE `EPERM` / `ENOTSUP` + docs |
| Internal | Corrupt WAL, invariant break | Log; refuse to mount/serve unrecovered dirty state |
| Query | Syntax error vs empty hits | Non-zero exit only on syntax/eval error; empty hits is success |

FUSE adapter must not panic across the kernel boundary: log and map to `EIO` when necessary.

---

## 9. Testing strategy

| Layer | Coverage |
| --- | --- |
| Unit | Canonical encoding round-trip; path rules; type op tables; AlefQL parser |
| Storage | Commit durability; truncated WAL / mid-batch crash simulation; reopen equality |
| Namespace | mkdir/put/unlink; nested structures; invariants |
| Query | Operator matrix per type; AND/OR/NOT; wrong-type → no match |
| FUSE integration | Mount temp dir; create via CLI, read via `cat`/`ls`; write via shell; xattr type |
| Golden docs | Documented supported FS edit paths covered by scripts |

**CI:** unit + storage + query on all PRs; FUSE integration on Linux runners only.

---

## 10. Technology choices

| Area | Choice |
| --- | --- |
| Language | Rust 2021 |
| FUSE | `fuser` |
| CLI | `clap` |
| Canonical on-disk encode | Custom compact binary v1 (versioned) |
| Logging | `tracing` |
| Primary OS | Linux (FUSE); macOS/`macFUSE` optional later |

---

## 11. Phased implementation plan

Critical path: **P0 → P1 → P2 → P3 → P4**. P5 may overlap late P3/P4. P6 is backlog.

| Phase | Deliverable | Exit criteria |
| --- | --- | --- |
| **P0 — Skeleton** | Workspace; `Value` types; encode/decode; path types; empty `Storage` trait | Tests for encode + paths |
| **P1 — Durable KV core** | S1 WAL storage; node graph for directories + **scalar** values; CLI get/set/mkdir | Restart preserves scalars |
| **P2 — Structure types** | Hash, set, list, tree in Value + namespace ops | Structure CRUD via CLI; nesting works |
| **P3 — FUSE mount** | Read-only mount first, then writes for documented paths | `ls`/`cat`/xattr match CLI; write scripts pass |
| **P4 — AlefQL** | Parser + evaluator with operators in §6 | Golden query tests; CLI `query` |
| **P5 — Hardening** | Crash tests; compaction S2; FUSE edge cases; export/import | Recovery suite green; disk growth bounded under a load script |
| **P6 — Research extensions** | Secondary indexes; multi-op transaction UX; richer tooling | As needed; not required for “usable research platform” |

### 11.1 Deferred explicitly

- Secondary indexes / query planner
- Cross-path interactive multi-statement transactions beyond explicit write batches
- Query-language writes
- Replication
- Full POSIX / multi-user ACL model

---

## 12. Key decisions

| Decision | Rationale |
| --- | --- |
| Layered engine + FUSE adapter | Dual interface without dual storage; each research concern is isolatable |
| Full type algebra early | Avoids painting path/query/FS design into a corner; implementation still phased |
| Explicit mkdir | Clearer namespace semantics for research and testing |
| Type-stable FUSE writes | Prevents silent type corruption from editors and shell redirects |
| Canonical encoding for equality | Well-defined sets and query equality |
| S1 WAL + memory index | Durability without delaying FUSE/types on a full LSM |
| Storage trait | Engine experiments without rewriting namespace/query/FUSE |
| AlefQL as AST-first language | Tests and CLI share semantics; planner can improve later |
| Wrong-type predicate = no match | Ergonomic composition; errors reserved for syntax/internal failures |
| Single-writer daemon v1 | Simpler invariants while FUSE + durability are proven |
| Linux + Rust + fuser | Strong systems fit for FUSE and correctness-oriented research |

---

## 13. Open questions

None blocking implementation planning. Defaults below apply unless revisited at the named phase:

1. **Admin transport (P1):** Unix socket (path under the data dir or `XDG_RUNTIME_DIR`), not TCP.
2. **Scalar file encoding on FUSE (P3):** UTF-8 text for Null/Bool/Int/Float/String; raw bytes for `Bytes`. Document both.
3. **Set member filenames (P3):** Short content hash as dirent name + xattr `user.alefs.member` holding display/canonical hint when needed.
4. **Project binary name:** `alefsdb`.

---

## 14. PR Plan

Incremental, reviewable slices aligned with phases. Each PR should be independently mergeable and tested at its layer.

| PR | Title | Components | Depends on | Description |
| --- | --- | --- | --- | --- |
| PR1 | Workspace skeleton and `alefs-types` | `alefs-types`, workspace `Cargo.toml`, README stub | — | Value enum, scalar variants, canonical encode/decode, unit tests |
| PR2 | Paths and namespace types (in-memory) | `alefs-namespace` (memory), path validation | PR1 | Path parsing/normalization, dir/value nodes in memory, mkdir/lookup tests |
| PR3 | `Storage` trait + S1 WAL engine | `alefs-storage` | — (can parallel PR1) | Trait, WAL commit/recover, crash/truncation tests |
| PR4 | Namespace on durable storage + scalar CLI | `alefs-namespace`, `alefs-cli`, `alefs-server` (no FUSE yet) | PR2, PR3 | Persist dirs/scalars; `serve` without mount or open-local CLI; get/set/mkdir |
| PR5 | Hash and list structures | types, namespace, CLI | PR4 | Nested hash/list CRUD |
| PR6 | Set and tree structures | types, namespace, CLI | PR5 | Set membership by canonical form; ordered tree keys |
| PR7 | FUSE read-only mount | `alefs-fuse`, server | PR6 | Mount; readdir/read/getattr/xattr match CLI |
| PR8 | FUSE writes (documented paths) | `alefs-fuse` | PR7 | Scalar writeback; structure create/unlink per policy |
| PR9 | AlefQL parser | `alefs-query` | PR1 | Parse to AST; syntax error tests |
| PR10 | AlefQL evaluator + CLI query | `alefs-query`, CLI | PR6, PR9 | Operators + AND/OR/NOT; golden tests (can start after PR4 for scalars, complete after PR6) |
| PR11 | Hardening: compaction S2 + recovery suite | storage, CI | PR4+ | Segment GC; expanded crash tests |
| PR12 | Export/import + supported-FS-edit docs | CLI, docs | PR8 | JSON/tooling export; user-facing edit-path documentation |

**Parallelism notes:** PR1 ∥ PR3 early; PR9 can start once types exist; PR10 full operator matrix needs PR6; FUSE PRs after structures exist to avoid rework.

---

## 15. Document history

| Date | Change |
| --- | --- |
| 2026-07-09 | Initial design from brainstorming (Approach 1, FUSE, durable, full types, AlefQL) |
