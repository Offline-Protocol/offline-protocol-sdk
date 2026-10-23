//! The Offline Protocol engine for the browser.
//!
//! A full mesh node, MLS end-to-end encryption included, compiled for
//! `wasm32-unknown-unknown` and driven from a dedicated Web Worker. The page
//! owns the carriers and moves bytes; [`node`] owns what the protocol says
//! about them. [`bindings`] is the JavaScript surface.
//!
//! Experimental: storage is in memory ([`memory_store`]), so an identity lasts
//! as long as the worker.
//!
//! The crate also builds on every host, so the workspace's lint, docs and test
//! runs cover it: everything below the bindings is plain Rust, and the tests
//! in [`node`] run two engines against each other without a browser.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod bindings;
pub mod memory_store;
pub mod node;

#[cfg(test)]
mod tests;
