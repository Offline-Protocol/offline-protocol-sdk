//! Main protocol engine.

use crate::{ProtocolConfig, Result};

/// Main entry point for the Offline Protocol SDK.
pub struct OfflineProtocol {
    #[allow(dead_code)]
    config: ProtocolConfig,
}

impl OfflineProtocol {
    /// Creates a new protocol instance.
    pub fn new(config: ProtocolConfig) -> Result<Self> {
        Ok(Self { config })
    }

    /// Starts the protocol.
    pub fn start(&mut self) -> Result<()> {
        // Placeholder - will be implemented in Phase 5
        Ok(())
    }

    /// Stops the protocol.
    pub fn stop(&mut self) -> Result<()> {
        // Placeholder - will be implemented in Phase 5
        Ok(())
    }
}
