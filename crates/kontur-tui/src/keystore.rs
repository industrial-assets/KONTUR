//! Persistent operator identity for bring-your-own-key joins.
//!
//! The operator's Ed25519 seed lives in a local key file that never leaves the
//! machine and is never printed. Only the derived public fingerprint is ever
//! shown (to read out-of-band so the host can approve it).

use std::io;
use std::path::PathBuf;

/// Directory holding the operator key (`~/.kontur`).
fn kontur_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory (set HOME)"))?;
    Ok(PathBuf::from(home).join(".kontur"))
}

/// Path to the operator key file.
pub fn key_path() -> io::Result<PathBuf> {
    Ok(kontur_dir()?.join("operator_key"))
}

/// Load the operator's 32-byte seed, generating and persisting a fresh one on
/// first use. The seed is written with owner-only permissions and is never
/// logged or printed — callers display only the fingerprint.
pub fn load_or_create_operator_seed() -> io::Result<[u8; 32]> {
    let path = key_path()?;
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return Ok(seed);
        }
        // Wrong size → treat as corrupt; refuse rather than silently regenerate,
        // so a stable identity is never lost to a stray file.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a 32-byte key file", path.display()),
        ));
    }

    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| io::Error::other(format!("getrandom: {e}")))?;
    std::fs::create_dir_all(kontur_dir()?)?;
    write_private(&path, &seed)?;
    Ok(seed)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, seed: &[u8; 32]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    use std::io::Write;
    f.write_all(seed)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, seed: &[u8; 32]) -> io::Result<()> {
    std::fs::write(path, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_persists_a_stable_seed() {
        // Use a temp HOME so we don't touch the real key file.
        let tmp = std::env::temp_dir().join(format!("kontur-keystore-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY: single-threaded test; we set HOME only for this process.
        std::env::set_var("HOME", &tmp);

        let a = load_or_create_operator_seed().unwrap();
        let b = load_or_create_operator_seed().unwrap();
        assert_eq!(a, b, "seed must be stable across loads");
        assert!(key_path().unwrap().exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
