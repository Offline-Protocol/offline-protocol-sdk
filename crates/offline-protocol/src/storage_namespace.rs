//! The account namespace: which directory, or which origin-private folder,
//! holds one protocol account's records.
//!
//! Every store that keeps an account apart from another derives its location
//! here, so a directory written by one binding opens under another for the
//! same account. Outside the `file-store` feature because the browser's store
//! uses it too, and a browser build has no POSIX files.

use sha2::{Digest, Sha256};

/// Domain separator of the account namespace digest. Shared with the Python,
/// Swift and Kotlin `StorageNamespace` helpers; changing it strands every
/// existing store.
const NAMESPACE_DOMAIN: &[u8] = b"offline-protocol-storage-v1";

/// Prefix of every account namespace.
const NAMESPACE_PREFIX: &str = "account-";

/// Returns the opaque, filesystem-safe namespace for one protocol account.
///
/// `account-` followed by the lowercase hex SHA-256 of
/// `"offline-protocol-storage-v1" || 0x00 || app_id || 0x00 || profile`, the
/// derivation every binding uses, so a directory written by one binding opens
/// under another for the same account.
pub fn account_storage_namespace(app_id: &str, profile: &str) -> String {
    let mut sha = Sha256::new();
    sha.update(NAMESPACE_DOMAIN);
    sha.update([0u8]);
    sha.update(app_id.as_bytes());
    sha.update([0u8]);
    sha.update(profile.as_bytes());
    format!("{NAMESPACE_PREFIX}{}", hex::encode(sha.finalize()))
}

/// Whether `value` is a well-formed account namespace.
///
/// The namespace becomes a path component, so an arbitrary string could
/// escape its own account (`..`) or collide with another's. Only the exact
/// shape [`account_storage_namespace`] produces is accepted: `account-` and
/// 64 lowercase hex digits.
pub fn is_account_storage_namespace(value: &str) -> bool {
    value.strip_prefix(NAMESPACE_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
