//! Router error types

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("No route available")]
    NoRouteAvailable,

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Message dropped: {0}")]
    MessageDropped(String),

    #[error("Core protocol error: {0}")]
    Core(#[from] offline_protocol_core::Error),

    #[error("Transport layer error: {0}")]
    TransportLayer(#[from] offline_protocol_transport::Error),
}

