# Examples

## Plugin manifest (`manifest.sample.json`)

Rough JSON shape matching [`Manifest`](../crates/jkr-api/src/manifest.rs) (built-in plugins use the same types in Rust).

Implement [`Plugin`](../crates/jkr-api/src/plugin.rs) in a crate linked into `jkr`, or ship a dynamic plugin following the repo’s plugin contract specs under `docs/superpowers/specs/` when those are checked in.

In-tree references: [`jkr-filter`](../crates/jkr-filter/), [`jkr-analytics`](../crates/jkr-analytics/).
