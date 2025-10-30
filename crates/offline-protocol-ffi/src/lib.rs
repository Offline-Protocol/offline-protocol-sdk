//! C FFI bindings for the Offline Protocol SDK.
//!
//! This crate exposes a C-compatible API for cross-platform interoperability.
//! This is the ONLY crate in the SDK that contains unsafe code, isolated to
//! FFI boundaries.
//!
//! # Safety
//!
//! All unsafe code in this crate is documented with SAFETY comments explaining
//! why the operations are safe. All pointers are validated before use, and
//! panics are caught to prevent unwinding across FFI boundaries.

#![warn(missing_docs)]

// Note: We allow unsafe_code only in this FFI crate
// All other crates have #![deny(unsafe_code)]
