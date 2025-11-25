//! Protocol-level constants.

/// Default time-to-live for messages.
pub const DEFAULT_INITIAL_TTL: u8 = 8;

/// Maximum number of messages to keep in outbox.
pub const MAX_OUTBOX_ENTRIES: usize = 500;

