//! Kontur core: the four-eyes dual-hold gate and tamper-evident audit chain.
//!
//! Pure, synchronous, no I/O. Time and signing are injected via traits.

pub mod ids;
pub mod verdict;

pub use ids::{GateId, HandEditRef, Hash, OperatorId, Sig, TaskId, Timestamp};
pub use verdict::{Remedy, ReviewDepth, Verdict};
