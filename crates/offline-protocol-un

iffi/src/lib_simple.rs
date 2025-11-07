//! Simplified UniFFI bindings - matching actual core API
//!
//! This is a minimal implementation that we'll expand incrementally.

#![allow(unsafe_code)] // Required for UniFFI generated scaffolding
#![warn(missing_docs)]

use offline_protocol::{
    OfflineProtocol as CoreProtocol, ProtocolConfig as CoreConfig,
};
use offline_protocol_core::MessagePriority as CorePriority;
use std::sync::Mutex;

// Include the UniFFI scaffolding  
uniffi::include_scaffolding!("offline_protocol_simple");

/// Error types for protocol operations
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Protocol not started
    #[error("Protocol not started")]
    NotStarted,
    
    /// Protocol already started
    #[error("Protocol already started")]
    AlreadyStarted,
    
    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
    
    /// Send operation failed
    #[error("Failed to send message: {0}")]
    SendFailed(String),
    
    /// Other error
    #[error("{0}")]
    Other(String),
}

impl From<offline_protocol::Error> for ProtocolError {
    fn from(err: offline_protocol::Error) -> Self {
        match err {
            offline_protocol::Error::NotStarted => ProtocolError::NotStarted,
            offline_protocol::Error::AlreadyStarted => ProtocolError::AlreadyStarted,
            offline_protocol::Error::InvalidConfiguration(msg) => {
                ProtocolError::InvalidConfiguration(msg)
            }
            _ => ProtocolError::Other(err.to_string()),
        }
    }
}

/// Message priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    Low,
    Medium,
    High,
    Critical,
}

impl From<MessagePriority> for CorePriority {
    fn from(priority: MessagePriority) -> Self {
        match priority {
            MessagePriority::Low => CorePriority::Low,
            MessagePriority::Medium => CorePriority::Medium,
            MessagePriority::High => CorePriority::High,
            MessagePriority::Critical => CorePriority::Critical,
        }
    }
}

/// Protocol configuration
#[derive(Debug, Clone)]
pub struct ProtocolConfig {
    pub app_id: String,
    pub user_id: String,
    pub ble_enabled: bool,
    pub wifi_direct_enabled: bool,
    pub internet_enabled: bool,
    pub prefer_online: bool,
    pub initial_ttl: u8,
}

impl From<ProtocolConfig> for CoreConfig {
    fn from(config: ProtocolConfig) -> Self {
        let mut core_config = CoreConfig::new(config.app_id, config.user_id);
        core_config.transport.ble_enabled = config.ble_enabled;
        core_config.transport.wifi_direct_enabled = config.wifi_direct_enabled;
        core_config.transport.internet_enabled = config.internet_enabled;
        core_config.dors.prefer_online = config.prefer_online;
        core_config.initial_ttl = config.initial_ttl;
        core_config
    }
}

/// Main protocol interface - simplified version
pub struct OfflineProtocol {
    inner: Mutex<CoreProtocol>,
}

impl OfflineProtocol {
    /// Creates a new protocol instance
    pub fn new(config: ProtocolConfig) -> Result<Self, ProtocolError> {
        let core_config: CoreConfig = config.into();
        core_config.validate().map_err(ProtocolError::from)?;
        
        let protocol = CoreProtocol::new(core_config).map_err(ProtocolError::from)?;
        
        Ok(Self {
            inner: Mutex::new(protocol),
        })
    }
    
    /// Starts the protocol
    pub fn start(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.start().map_err(ProtocolError::from)
    }
    
    /// Stops the protocol
    pub fn stop(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.stop().map_err(ProtocolError::from)
    }
    
    /// Pauses the protocol
    pub fn pause(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.pause().map_err(ProtocolError::from)
    }
    
    /// Resumes the protocol
    pub fn resume(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.resume().map_err(ProtocolError::from)
    }
    
    /// Process internal protocol operations
    pub fn process(&self) -> Result<(), ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.process().map_err(ProtocolError::from)
    }
    
    /// Sends a message
    pub fn send_message(
        &self,
        recipient: String,
        content: String,
        priority: MessagePriority,
    ) -> Result<String, ProtocolError> {
        let mut protocol = self.inner.lock().unwrap();
        let message_id = protocol
            .send_message(&recipient, &content, Some(priority.into()))
            .map_err(|e| ProtocolError::SendFailed(e.to_string()))?;
        
        Ok(message_id.as_str())
    }
    
    /// Receives the next message (returns JSON string or None)
    pub fn receive_message(&self) -> Option<String> {
        let mut protocol = self.inner.lock().unwrap();
        protocol.receive_message().and_then(|msg| {
            serde_json::to_string(&msg).ok()
        })
    }
}

