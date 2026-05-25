//! jkr-model — IPFS-backed model distribution + local serving.
//!
//! Three responsibilities:
//!
//! 1. **Manifests** ([`manifest`]) — describe a model as a set of content-addressed
//!    blobs (CIDs) plus runtime hints. Manifests are themselves content-addressed.
//! 2. **Registry** ([`registry`]) — maps human names (`"llama-3.1-8b-q4"`) to
//!    manifest CIDs. Implementation gossips over the existing jkr-mesh transport,
//!    so peers learn what other peers have published.
//! 3. **Pull / Run / Serve** — fetch blobs over iroh, hand them to a bundled
//!    llama.cpp runner, expose an OpenAI-compatible HTTP surface.

pub mod manifest;
pub mod registry;
// pub mod pull;     // iroh blob fetch — wire after manifest shape is settled.
// pub mod runner;   // llama-cpp-2 wrapper — gated behind cuda/metal/vulkan features.
// pub mod serve;    // OpenAI-compat HTTP shim — depends on runner.

pub use manifest::Manifest;
