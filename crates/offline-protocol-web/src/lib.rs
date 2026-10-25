//! The Offline Protocol engine for the browser.
//!
//! A full mesh node, MLS end-to-end encryption included, compiled for
//! `wasm32-unknown-unknown` and driven from a dedicated Web Worker. The page
//! owns the carriers and moves bytes; [`node`] owns what the protocol says
//! about them. [`bindings`] is the JavaScript surface.
//!
//! Storage is durable: [`log_store`] keeps each category in a sealed append
//! log over origin-private files ([`opfs`]), so an identity outlives the
//! worker. [`memory_store`] remains for a node that should not.
//!
//! [`signal`] lets two pages that share a room code exchange their WebRTC
//! connection codes through Nostr relays instead of by hand, and
//! [`nostr_host`] drives the Nostr transport itself from the worker, with
//! relays the application names.
//!
//! The crate also builds on every host, so the workspace's lint, docs and test
//! runs cover it: everything below the bindings is plain Rust, and the tests
//! in [`node`] run two engines against each other without a browser.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod bindings;
pub mod log_store;
pub mod memory_store;
pub mod node;
pub mod nostr_host;
pub mod opfs;
pub mod signal;

#[cfg(test)]
mod nostr_tests;
#[cfg(test)]
mod tests;
