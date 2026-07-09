# alefsdb

Research-oriented **typed structure database** that also mounts as a real **FUSE filesystem**, with a structured query language (AlefQL).

See the design:

- [Design document](docs/superpowers/specs/2026-07-09-alefsdb-design.md)
- [P0 plan](docs/superpowers/plans/2026-07-09-alefsdb-p0-skeleton.md)

## Status

**P0 — Skeleton** (complete): value types, canonical codec, paths, storage trait.

## Build & test

```bash
cargo test
```

Requires Rust 1.70+ (stable) on Linux for later FUSE work.

## Crates

| Crate | Role |
| --- | --- |
| `alefs-types` | `Value` algebra, canonical encode/decode, `DbPath` |
| `alefs-storage` | `Storage` trait + in-memory (S0) backend |

## License

MIT (unless otherwise noted).
