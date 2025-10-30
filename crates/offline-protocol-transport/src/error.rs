//! Transport error types

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Transport not started")]
    NotStarted,

    #[error("Transport already started")]
    AlreadyStarted,

    #[error("BLE error: {0}")]
    Ble(String),

    #[error("Wi-Fi Direct error: {0}")]
    WiFiDirect(String),

    #[error("Send failed: {0}")]
    SendFailed(String),

    #[error("Receive failed: {0}")]
    ReceiveFailed(String),

    #[error("Neighbor not found: {0}")]
    NeighborNotFound(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Core protocol error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<btleplug::Error> for Error {
    fn from(err: btleplug::Error) -> Self {
        Error::Ble(err.to_string())
    }
}

