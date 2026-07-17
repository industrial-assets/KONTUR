use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// An operator's stable identity: their Ed25519 public key bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct OperatorId(pub [u8; 32]);

/// Identifier for a gate (one per gated action).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct GateId(pub String);

/// Identifier for a task in the plan.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TaskId(pub String);

/// Reference to a direct human change (a hand-edit).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HandEditRef(pub String);

/// Milliseconds since the Unix epoch. Supplied by the injected `Clock`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Timestamp(pub i64);

/// A 32-byte SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

/// A 64-byte Ed25519 signature. `serde` only derives arrays up to length 32, so
/// the 64-byte field uses `serde-big-array`. `[u8; 64]` implements
/// `PartialEq`/`Eq`/`Debug`/`Copy` for all N in std, so those still derive.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Sig(#[serde(with = "BigArray")] pub [u8; 64]);
