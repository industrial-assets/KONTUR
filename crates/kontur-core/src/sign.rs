use ed25519_dalek::{Signer as _, SigningKey, Verifier as _, VerifyingKey};

use crate::ids::{OperatorId, Sig, Timestamp};

/// Produces signed verdicts. In production an operator's key lives on their
/// station; in tests we construct one from a fixed seed for determinism.
pub trait Signer {
    fn operator_id(&self) -> OperatorId;
    fn sign(&self, msg: &[u8]) -> Sig;
}

/// Injected time source — the core never reads the wall clock.
pub trait Clock {
    fn now(&self) -> Timestamp;
}

/// Ed25519 signer built from a 32-byte seed (deterministic; no RNG).
pub struct Ed25519Signer {
    key: SigningKey,
}

impl Ed25519Signer {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Ed25519Signer {
            key: SigningKey::from_bytes(&seed),
        }
    }
}

impl Signer for Ed25519Signer {
    fn operator_id(&self) -> OperatorId {
        OperatorId(self.key.verifying_key().to_bytes())
    }

    fn sign(&self, msg: &[u8]) -> Sig {
        Sig(self.key.sign(msg).to_bytes())
    }
}

/// Verify a signature against the public key embedded in `op`. Returns false on
/// any malformed key/signature — never panics.
pub fn verify(op: OperatorId, msg: &[u8], sig: &Sig) -> bool {
    let vk = match VerifyingKey::from_bytes(&op.0) {
        Ok(vk) => vk,
        Err(_) => return false,
    };
    let signature = ed25519_dalek::Signature::from_bytes(&sig.0);
    vk.verify(msg, &signature).is_ok()
}

/// A clock that always returns the same instant — for deterministic tests.
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let signer = Ed25519Signer::from_seed([1u8; 32]);
        let op = signer.operator_id();
        let msg = b"gate-03 diff-hash go";
        let sig = signer.sign(msg);
        assert!(verify(op, msg, &sig));
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let signer = Ed25519Signer::from_seed([1u8; 32]);
        let op = signer.operator_id();
        let sig = signer.sign(b"original");
        assert!(!verify(op, b"tampered", &sig));
    }

    #[test]
    fn verify_rejects_wrong_operator() {
        let a = Ed25519Signer::from_seed([1u8; 32]);
        let b = Ed25519Signer::from_seed([2u8; 32]);
        let msg = b"msg";
        let sig = a.sign(msg);
        assert!(!verify(b.operator_id(), msg, &sig));
    }

    #[test]
    fn distinct_seeds_give_distinct_identities() {
        let a = Ed25519Signer::from_seed([1u8; 32]);
        let b = Ed25519Signer::from_seed([2u8; 32]);
        assert_ne!(a.operator_id(), b.operator_id());
    }
}
