//! Kontur core: the four-eyes dual-hold gate and tamper-evident audit chain.
//!
//! Pure, synchronous, no I/O. Time and signing are injected via traits.

pub mod canonical;
pub mod ids;
pub mod sign;
pub mod verdict;

pub use canonical::{canonical_bytes, sha256};
pub use ids::{GateId, HandEditRef, Hash, OperatorId, Sig, TaskId, Timestamp};
pub use sign::{verify, Clock, Ed25519Signer, Signer};
pub use verdict::{Remedy, ReviewDepth, Verdict};
