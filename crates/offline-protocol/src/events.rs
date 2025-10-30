//! Event types and callbacks.

/// Event types that can occur in the protocol.
#[derive(Debug, Clone)]
pub enum Event {
    // Placeholder - will be implemented in Phase 5
}

/// Event callback type.
pub type EventCallback = Box<dyn Fn(Event) + Send + Sync>;
