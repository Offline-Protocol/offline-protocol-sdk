//! Error types for the SDK

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Protocol not started")]
    NotStarted,

    #[error("Protocol already started")]
    AlreadyStarted,

    #[error("Send failed: {0}")]
    SendFailed(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("File transfer error: {0}")]
    FileTransfer(String),

    #[error("Core error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    #[error("Transport error: {0}")]
    Transport(#[from] offline_protocol_transport::Error),

    #[error("Router error: {0}")]
    Router(#[from] offline_protocol_router::Error),

    #[error("Reliability error: {0}")]
    Reliability(#[from] offline_protocol_reliability::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

