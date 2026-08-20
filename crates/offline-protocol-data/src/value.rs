//! Values that can be stored in a document collection.

use serde::{Deserialize, Serialize};

/// A value stored in a map or list.
///
/// Deliberately flat. Structured values go in as JSON strings and merge
/// whole (last write wins per key), which is the honest description of what
/// v1 replicates: whole collections within a space, no nested container
/// addressing and no query language. [`DataValue::Attachment`] is the one
/// composite, and it is composite because it is a reference rather than a
/// value: see its own documentation.
///
/// The engine's own value type never appears here. That is what lets the
/// engine be replaced without a breaking change to any caller, the FFI, or
/// any binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DataValue {
    /// An explicit null.
    Null,
    /// A boolean.
    Bool {
        /// The value.
        value: bool,
    },
    /// A signed 64-bit integer.
    Int {
        /// The value.
        value: i64,
    },
    /// A double.
    Float {
        /// The value.
        value: f64,
    },
    /// A UTF-8 string.
    Text {
        /// The value.
        value: String,
    },
    /// Opaque bytes.
    Bytes {
        /// The value.
        value: Vec<u8>,
    },
    /// A reference to a blob that is not in the document.
    ///
    /// The only value whose payload never enters the CRDT. The document
    /// holds the hash, the size, and enough to show the thing; the bytes
    /// travel on the media path that already carries files.
    ///
    /// The failure this prevents is a document that has to hold a photo. A
    /// document is bounded by one sealed record, so a layer that inlined
    /// blobs would be a layer that could not carry the blobs people
    /// actually send, and would discover that at commit time with the
    /// picture already on screen.
    ///
    /// A reference is replaced, never edited. Two members who attach
    /// different blobs to the same key resolve the way every other map
    /// value resolves, and neither replica is left holding half of each.
    /// That is what the "no multi-writer attachment mutation" non-goal
    /// means in practice, and it is a property of this shape rather than a
    /// rule anyone has to remember.
    Attachment {
        /// Lowercase hex SHA-256 of the whole blob, exactly 64 characters.
        ///
        /// This is the address, not a checksum carried alongside one. The
        /// same bytes attached twice are the same attachment, and a blob
        /// arriving from a peer is accepted only if it hashes to the
        /// reference that asked for it, which is what makes fetching from
        /// an authenticated peer safe without trusting that peer.
        hash: String,
        /// Length of the blob in bytes.
        ///
        /// Carried so an application can decide whether it wants the thing
        /// before asking for it over a radio that may be Bluetooth.
        size: u64,
        /// Display name, if the writer had one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Media type, if the writer knew it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
    },
}

impl DataValue {
    /// Convenience constructor for a string value.
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text {
            value: value.into(),
        }
    }

    /// Convenience constructor for an integer value.
    pub fn int(value: i64) -> Self {
        Self::Int { value }
    }

    /// Convenience constructor for a boolean value.
    pub fn bool(value: bool) -> Self {
        Self::Bool { value }
    }

    /// Convenience constructor for a float value.
    pub fn float(value: f64) -> Self {
        Self::Float { value }
    }

    /// Convenience constructor for a byte value.
    pub fn bytes(value: impl Into<Vec<u8>>) -> Self {
        Self::Bytes {
            value: value.into(),
        }
    }

    /// Convenience constructor for an attachment reference with no
    /// display name and no media type.
    ///
    /// The hash is not checked here. It is checked where every other value
    /// is checked, at the operation that writes it, so a bad one is refused
    /// with the same error as a bad key rather than panicking at a
    /// constructor.
    pub fn attachment(hash: impl Into<String>, size: u64) -> Self {
        Self::Attachment {
            hash: hash.into(),
            size,
            name: None,
            mime: None,
        }
    }
}
