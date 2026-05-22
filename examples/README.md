# Examples

## Plugin manifest (`manifest.sample.json`)

Rough JSON shape matching [`Manifest`](../crates/tkr-api/src/manifest.rs) (built-in plugins use the same types in Rust).

Implement [`Plugin`](../crates/tkr-api/src/plugin.rs) in a crate linked into `tkr`, or ship a dynamic plugin following the repo’s plugin contract specs under `docs/superpowers/specs/` when those are checked in.

In-tree references: [`tkr-filter`](../crates/tkr-filter/), [`tkr-analytics`](../crates/tkr-analytics/).
