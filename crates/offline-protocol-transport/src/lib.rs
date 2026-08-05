//! Transport abstraction layer for the Offline Protocol SDK.
//!
//! This crate defines the [`Transport`] trait and the transport state
//! machines for BLE, Wi-Fi Direct, Internet, Nostr, and Reticulum.
//!
//! # This crate performs no network or radio I/O
//!
//! No sockets are opened and no radios are touched here. Each transport is
//! a thread-safe protocol/queue engine — status, send/receive queues,
//! metrics, and (for BLE) fragmentation state — driven from two sides:
//!
//! - **The protocol engine** talks to the [`Transport`] trait: `send()`
//!   enqueues an outbound message, `receive()` dequeues an inbound one.
//! - **A platform bridge** (Swift/Kotlin via the UniFFI bindings, or any
//!   host program) owns the actual sockets, relays, and radios. It drains
//!   the outbound queue (`get_next_message()` / `get_next_signed_event()` /
//!   `get_next_fragment()`), performs the real I/O, reports the outcome
//!   (`confirm_sent()` / `report_send_failure()`), injects inbound bytes
//!   (`on_data_received()` / `on_fragment_received()`), and drives
//!   availability (`on_status_changed()`).
//!
//! Without a platform bridge attached, `send()` queues messages that never
//! go anywhere — and, BLE excepted, `start()` never makes a transport
//! `Available` in the first place, so `send()` returns
//! `TransportNotAvailable`. A pure-Rust consumer must play the bridge role
//! itself: mark the transport available, drain the queues, and do its own
//! I/O.
//!
//! Most transports signal pending outbound work through a
//! `set_on_messages_available` callback instead of requiring a poll timer;
//! [`InternetTransport`] is the exception and must be polled.
//!
//! All code in this crate is 100% safe Rust with no unsafe blocks.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ble;
pub(crate) mod common;
pub mod constants;
pub mod error;
pub mod internet;
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;
// NIP-44 v2 is an implementation detail of the Nostr gift wrap: nothing outside
// this crate should be able to reach the primitives directly, and in particular
// nothing should be able to hand it a conversation key of its own choosing.
mod nip44;
pub mod nostr;
pub mod nostr_crypto;
pub mod reticulum;
pub mod traits;
pub mod types;
pub mod wifi_direct;

pub use ble::{
    BleTransport, BleTransportBuilder, FragmentEvictionCallback, FragmentEvictionInfo, PeerDevice,
};
pub use constants::DEFAULT_MAX_MESSAGE_SIZE;
pub use error::{Error, Result};
pub use internet::{InternetConfig, InternetTransport};
pub use nostr::{NostrConfig, NostrTransport, NostrTransportBuilder, SignedNostrEvent};
pub use nostr_crypto::{routing_tag_for_device_id, NostrEvent, NostrKeypair};
pub use reticulum::{ReticulumConfig, ReticulumTransport};
pub use traits::{Transport, TransportStatus};
pub use types::{LinkQuality, SharedCallback, TransportMetrics, TransportType};
pub use wifi_direct::{WifiDirectConfig, WifiDirectPeer, WifiDirectTransport};

// MockTransport is only available to this crate's tests or explicit test consumers.
#[cfg(any(test, feature = "test-utils"))]
pub use mock::MockTransport;
