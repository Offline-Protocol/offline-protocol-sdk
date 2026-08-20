#![deny(unsafe_code)]
#![warn(missing_docs)]

//! Replicated documents for the Offline Protocol SDK.
//!
//! Messaging is synced events; this crate is synced state. A document is a
//! set of collections that any member of a space can edit while offline,
//! merging deterministically when the replicas meet again.
//!
//! # What this crate deliberately does not do
//!
//! It does not persist, encrypt, or send anything. It turns edits into
//! opaque byte deltas and turns byte deltas back into state, and that is
//! all. Storage, sealing and delivery belong to the main `offline-protocol`
//! crate, which already carries ten releases of reliability work this layer
//! inherits for free.
//!
//! # The engine is an implementation detail
//!
//! A CRDT engine is embedded here and named nowhere in this crate's public
//! API. No engine type appears in a signature, in the FFI surface, or in any
//! binding. One leaked engine type and the engine could never be replaced
//! without a breaking release everywhere, which is exactly the lock-in this
//! layer exists to avoid.

/// Document, collections, deltas and encoding.
pub mod doc;
/// Error types.
pub mod error;
/// Size caps and the compaction trigger.
pub mod policy;
/// Values stored in collections.
pub mod value;

pub use doc::{
    BlobMeta, CatchUp, DataDoc, Delta, DocStats, ImportOutcome, RemoteImport, VersionToken,
};
pub use error::{DataError, DataResult};
pub use policy::{
    should_compact, size_verdict, SizeVerdict, COMPACT_DELTA_LOG_RATIO, COMPACT_MAX_COMMITS,
    COMPACT_MIN_DELTA_LOG_BYTES, DOC_SIZE_WARN_BYTES, MAX_DOC_BYTES,
};
pub use value::DataValue;

/// Longest accepted name for a space, document or collection.
pub const MAX_NAME_LEN: usize = 128;

/// Longest accepted map key.
pub const MAX_KEY_LEN: usize = 256;

/// Number of hex characters in an attachment hash, a SHA-256.
pub const ATTACHMENT_HASH_LEN: usize = 64;

/// Longest accepted attachment display name.
///
/// Shorter than a file system's limit on purpose. This is a label shown
/// next to the thing, not a path anything opens, and it replicates to every
/// member of the space whether or not they ever fetch the blob.
pub const MAX_ATTACHMENT_NAME_LEN: usize = 256;

/// Longest accepted attachment media type.
///
/// RFC 6838 bounds a type and a subtype at 127 characters each, so this
/// accepts every registered name and leaves nothing to argue about.
pub const MAX_ATTACHMENT_MIME_LEN: usize = 255;

/// Largest size an attachment reference may declare.
///
/// Not a statement about how large a blob may be: what a transport will
/// carry is the transfer layer's business and is far smaller than this. It
/// is the largest value the engine can hold without changing meaning, since
/// the engine has one integer type and it is signed. A reference declaring
/// more is refused at the operation that writes it rather than clamped on
/// the way in, because a clamped size is a size its writer never wrote.
pub const MAX_ATTACHMENT_SIZE: u64 = i64::MAX as u64;

/// Longest accepted single value, equal to the whole-document cap.
///
/// A value at or past this size can never fit inside a document that also
/// has to hold its own framing, so accepting one only defers the refusal to
/// a place where it is expensive: an unbounded value makes a commit whose
/// delta record cannot be written at all, and a delta that cannot be written
/// is re-exported larger at every retry. Refusing at the operation keeps that
/// failure in front of the caller, while the edit is still theirs to change.
pub const MAX_VALUE_BYTES: usize = policy::MAX_DOC_BYTES;

/// Check a space, document, or collection name.
///
/// The charset is narrow on purpose. These names are composed into storage
/// record keys (`{space}/{doc}`), so a name containing a separator or a
/// path traversal would make a key that parses back into different parts
/// than it was built from. Rejecting the character is the cheap half of
/// that problem; the expensive half is discovering it after records exist.
pub fn validate_name(name: &str) -> DataResult<()> {
    if name.is_empty() {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "must not be empty",
        });
    }
    if name.len() > MAX_NAME_LEN {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "longer than MAX_NAME_LEN",
        });
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "may only contain A-Z a-z 0-9 . _ -",
        });
    }
    Ok(())
}

/// Check a space name.
///
/// Looser than [`validate_name`] by exactly one character, the colon, and
/// the reason is that a space is named after the MLS scope it replicates
/// in. A 1:1 space is named by a peer address; a group space is named by
/// the group id, which is minted as `group:<uuid>`. Refusing the colon
/// would mean either giving group spaces a second, translated name — one
/// more mapping to keep consistent and to get wrong — or refusing group
/// replication outright.
///
/// The colon is safe here in a way it would not be in a document name.
/// Record keys are composed as `{space}/{doc}` and `{space}/{doc}/{seq}`
/// and are parsed by stripping the space prefix and splitting the
/// remainder on `/`, so only the separator itself can make a key parse
/// back into different parts than it was built from, and that stays
/// forbidden.
///
/// Document names deliberately keep the narrow charset: a peer names those
/// on the wire, and this one is never wire-supplied. A 1:1 space is derived
/// from the authenticated sender of the frame and a group space from the
/// group whose key decrypted it, so a peer cannot choose either.
pub fn validate_space_name(name: &str) -> DataResult<()> {
    if name.is_empty() {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "must not be empty",
        });
    }
    if name.len() > MAX_NAME_LEN {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "longer than MAX_NAME_LEN",
        });
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
    {
        return Err(DataError::InvalidName {
            name: name.to_string(),
            reason: "may only contain A-Z a-z 0-9 . _ - :",
        });
    }
    Ok(())
}

/// Check a map key.
///
/// Keys are looser than names because they never enter a record key: they
/// live inside the document. The only requirements are non-empty and
/// bounded, so a single key cannot consume a meaningful share of the
/// document's byte budget on its own.
pub fn validate_key(key: &str) -> DataResult<()> {
    if key.is_empty() {
        return Err(DataError::InvalidName {
            name: key.to_string(),
            reason: "must not be empty",
        });
    }
    if key.len() > MAX_KEY_LEN {
        return Err(DataError::InvalidName {
            name: key.to_string(),
            reason: "longer than MAX_KEY_LEN",
        });
    }
    Ok(())
}

/// Check the size of a single value.
///
/// Bounds the one input the caller controls without limit. Note this is a
/// per-value bound only: the whole-document cap still applies, and is what
/// catches many small values adding up.
pub fn validate_value(value: &DataValue) -> DataResult<()> {
    let len = match value {
        DataValue::Text { value } => value.len(),
        DataValue::Bytes { value } => value.len(),
        // An attachment is bounded by its own rules, which are tighter than
        // the value cap and check shape rather than size: `size` describes
        // bytes that are somewhere else, so the value cap says nothing
        // useful about it.
        DataValue::Attachment {
            hash,
            size,
            name,
            mime,
        } => return validate_attachment(hash, *size, name.as_deref(), mime.as_deref()),
        // Scalars are fixed-width and cannot breach any bound.
        DataValue::Null
        | DataValue::Bool { .. }
        | DataValue::Int { .. }
        | DataValue::Float { .. } => 0,
    };
    validate_value_len(len)
}

/// Check that a string is a well-formed attachment address.
///
/// The hash is checked for shape, not for reachability: whether anybody
/// still holds the bytes is not knowable here, and a reference to a blob
/// nobody has is a normal, recoverable state. What is not recoverable is a
/// hash that is not a hash, because no fetch can ever succeed and the
/// reference replicates to the whole space regardless.
///
/// Case matters. The hash is an address, and two spellings of one address
/// are two addresses: they would fetch twice, store twice, and compare
/// unequal while naming identical bytes. Lowercase is the canonical form,
/// and the uppercase spelling is refused rather than folded so that a
/// writer producing the wrong one hears about it.
///
/// Separate from [`validate_attachment`] because a hash also travels alone:
/// the frames that ask for a blob and refuse one carry nothing else. Checking
/// those through the whole-reference validator means inventing a size to
/// satisfy it, which ties a frame check to a bound that has nothing to do
/// with it and breaks the day that validator gains a cross-field rule.
pub fn validate_attachment_hash(hash: &str) -> DataResult<()> {
    if hash.len() != ATTACHMENT_HASH_LEN {
        return Err(DataError::InvalidAttachment {
            reason: "hash must be exactly 64 characters",
        });
    }
    if !hash
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(DataError::InvalidAttachment {
            reason: "hash must be lowercase hex",
        });
    }
    Ok(())
}

/// Check an attachment reference.
///
/// Every bound this layer places on a reference, in one place, so that the
/// write path and the read path cannot disagree about what a valid reference
/// is. A reference that fails any of them reads as absent rather than as an
/// error on the read path: see [`crate::DataDoc`].
pub fn validate_attachment(
    hash: &str,
    size: u64,
    name: Option<&str>,
    mime: Option<&str>,
) -> DataResult<()> {
    validate_attachment_hash(hash)?;
    if size == 0 {
        return Err(DataError::InvalidAttachment {
            reason: "size must not be zero",
        });
    }
    // The upper bound is structural rather than a policy about how large a
    // blob may be: the engine has one integer type and it is signed, so a
    // size past this cannot be stored without changing it. Refused here so
    // that the conversion further down is lossless by construction. Left to
    // the conversion, the same value would be silently clamped and a replica
    // would read back a size its writer never wrote.
    if size > MAX_ATTACHMENT_SIZE {
        return Err(DataError::InvalidAttachment {
            reason: "size is larger than MAX_ATTACHMENT_SIZE",
        });
    }
    if name.is_some_and(|name| name.len() > MAX_ATTACHMENT_NAME_LEN) {
        return Err(DataError::InvalidAttachment {
            reason: "name is longer than MAX_ATTACHMENT_NAME_LEN",
        });
    }
    if mime.is_some_and(|mime| mime.len() > MAX_ATTACHMENT_MIME_LEN) {
        return Err(DataError::InvalidAttachment {
            reason: "mime is longer than MAX_ATTACHMENT_MIME_LEN",
        });
    }
    Ok(())
}

/// Check a value length against [`MAX_VALUE_BYTES`].
///
/// Split out for text insertions, which carry a `&str` rather than a
/// [`DataValue`].
pub fn validate_value_len(len: usize) -> DataResult<()> {
    if len > MAX_VALUE_BYTES {
        return Err(DataError::ValueTooLarge {
            actual: len,
            limit: MAX_VALUE_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
