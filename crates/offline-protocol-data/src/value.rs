//! Values that can be stored in a document collection.

use serde::{Deserialize, Serialize};

/// A scalar stored in a map or list.
///
/// Deliberately scalar-only. Structured values go in as JSON strings and
/// merge whole (last write wins per key), which is the honest description
/// of what v1 replicates: whole collections within a space, no nested
/// container addressing and no query language.
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
}
