//! tkr-mesh — peer messaging mesh for tkr agents.
//!
//! Identity is a secp256k1 keypair (same shape as an Ethereum wallet).
//! The on-mesh address is the EIP-55 checksummed Ethereum address derived
//! from the public key, so a peer's mesh address and its on-chain address
//! are identical — no binding attestations needed for payments.

pub mod address;
pub mod attestation;
pub mod client;
pub mod enrollment;
pub mod envelope;
pub mod frames;
pub mod identity;
pub mod invite;
pub mod payment;

pub use client::Client;
pub use payment::{EscrowDomain, PaidMessage, Receipt};

pub use address::Address;
pub use attestation::{JoinAttestation, JOIN_ATTESTATION_MAX_SKEW_MS};
pub use enrollment::{enroll, JoinedMesh};
pub use envelope::Envelope;
pub use identity::Identity;
pub use invite::{Invite, Role};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("invalid invite: {0}")]
    InvalidInvite(String),
    #[error("signature verification failed")]
    BadSignature,
    #[error("invite expired (expires_at={expires_at}, now={now})")]
    Expired { expires_at: u64, now: u64 },
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("encoding: {0}")]
    Encoding(String),
}

pub type Result<T> = std::result::Result<T, Error>;
