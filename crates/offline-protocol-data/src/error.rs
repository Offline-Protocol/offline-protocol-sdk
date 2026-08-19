//! Errors raised by the data layer.

use thiserror::Error;

/// Result alias for data-layer operations.
pub type DataResult<T> = std::result::Result<T, DataError>;

/// A failure inside the replicated-document layer.
///
/// Every variant is either a caller mistake (a bad key, an out-of-range
/// position, a document over the cap) or a permanent corruption verdict.
/// Transient conditions do not appear here: persistence retries live above
/// this crate, on the SDK's storage seam.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DataError {
    /// A document, snapshot, or delta could not be decoded.
    ///
    /// The bytes reaching this crate always come back out of a sealed
    /// protocol-state record, so an AEAD tag has already vouched for them.
    /// This variant therefore means the engine rejected its own output:
    /// treat the document as permanently unreadable rather than retrying.
    #[error("document data is corrupt: {0}")]
    Corrupt(String),

    /// A document handle was used after a decode panic poisoned it.
    ///
    /// Held separate from [`DataError::Corrupt`] because the failure is
    /// sticky: every later call on the same handle answers this, and the
    /// only recovery is to drop the handle and re-open from storage.
    #[error("document handle is poisoned by an earlier decode failure")]
    Poisoned,

    /// The compacted document would exceed [`crate::MAX_DOC_BYTES`].
    ///
    /// Raised at commit, never as a silent truncation: an application that
    /// keeps typing into a full document has to hear about it while the
    /// text is still on screen.
    #[error("document is {actual} bytes compacted, over the {limit} byte cap")]
    DocTooLarge {
        /// Size of the compacted encoding that breached the cap.
        actual: usize,
        /// The cap in force, [`crate::MAX_DOC_BYTES`].
        limit: usize,
    },

    /// A single value is larger than [`crate::MAX_VALUE_BYTES`].
    ///
    /// Refused at the operation rather than at commit. A value this size can
    /// never fit in a document, and discovering that at commit means the
    /// document's next delta record is unwritable and stays unwritable,
    /// because a refused write is re-exported (larger) by the commit after it.
    #[error("value is {actual} bytes, over the {limit} byte limit")]
    ValueTooLarge {
        /// Size of the rejected value.
        actual: usize,
        /// The limit in force, [`crate::MAX_VALUE_BYTES`].
        limit: usize,
    },

    /// A collection or document name is empty, over-long, or uses characters
    /// outside the accepted set.
    #[error("invalid name {name:?}: {reason}")]
    InvalidName {
        /// The rejected name.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A list or text position is outside the current length.
    #[error("position {position} is out of range for length {length}")]
    OutOfRange {
        /// The position the caller asked for.
        position: usize,
        /// The current length of the collection.
        length: usize,
    },

    /// The engine refused an otherwise well-formed operation.
    #[error("data operation failed: {0}")]
    Engine(String),
}
