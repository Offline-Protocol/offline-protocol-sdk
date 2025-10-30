//! Reliability layer error types

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Message not found: {0}")]
    MessageNotFound(String),

    #[error("ACK timeout")]
    AckTimeout,

    #[error("Max retries exceeded")]
    MaxRetriesExceeded,

    #[error("Queue full")]
    QueueFull,

    #[error("Core protocol error: {0}")]
    Core(#[from] offline_protocol_core::Error),
}

