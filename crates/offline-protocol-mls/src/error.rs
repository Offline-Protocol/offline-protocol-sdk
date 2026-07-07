//! Error types for MLS operations.

use thiserror::Error;

/// Result type alias for MLS operations.
pub type Result<T> = std::result::Result<T, MlsError>;

/// Errors that can occur during MLS operations.
#[derive(Debug, Error)]
pub enum MlsError {
    /// Storage operation failed.
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    /// Failed to generate cryptographic material.
    #[error("Crypto generation failed: {0}")]
    CryptoGeneration(String),

    /// Failed to create a credential.
    #[error("Credential creation failed: {0}")]
    CredentialCreation(String),

    /// Failed to create a key package.
    #[error("Key package creation failed: {0}")]
    KeyPackageCreation(String),

    /// Invalid key package received.
    #[error("Invalid key package: {0}")]
    InvalidKeyPackage(String),

    /// Failed to create a group.
    #[error("Group creation failed: {0}")]
    GroupCreation(String),

    /// Group not found.
    #[error("Group not found: {0}")]
    GroupNotFound(String),

    /// Failed to add member to group.
    #[error("Failed to add member: {0}")]
    AddMember(String),

    /// Failed to remove member from group.
    #[error("Failed to remove member: {0}")]
    RemoveMember(String),

    /// Encryption failed.
    #[error("Encryption failed: {0}")]
    Encryption(String),

    /// Decryption failed.
    #[error("Decryption failed: {0}")]
    Decryption(String),

    /// Failed to process Welcome message.
    #[error("Welcome processing failed: {0}")]
    WelcomeProcessing(String),

    /// Failed to serialize data.
    #[error("Serialization failed: {0}")]
    Serialization(String),

    /// Failed to deserialize data.
    #[error("Deserialization failed: {0}")]
    Deserialization(String),

    /// MLS manager not initialized.
    #[error("MLS not initialized")]
    NotInitialized,

    /// No key package available for the user.
    #[error("No key package available for user: {0}")]
    NoKeyPackage(String),

    /// Session not found for 1:1 messaging.
    #[error("Session not found for user: {0}")]
    SessionNotFound(String),

    /// Invalid message format.
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// User not found in group.
    #[error("User not found in group: {0}")]
    UserNotInGroup(String),

    /// OpenMLS library error.
    #[error("OpenMLS error: {0}")]
    OpenMls(String),

    /// Signing operation failed.
    #[error("Signing failed: {0}")]
    Signing(String),

    /// Signature verification failed.
    #[error("Signature verification failed: {0}")]
    VerificationFailed(String),

    /// Invalid public key format.
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    /// Group id failed storage-key validation (empty, oversized, or
    /// containing path-traversal / storage-hostile characters).
    #[error("Invalid group id: {0}")]
    InvalidGroupId(String),

    /// User id failed storage-key validation (empty, or containing
    /// path-traversal / storage-hostile characters).
    #[error("Invalid user id: {0}")]
    InvalidUserId(String),

    /// A key package's embedded credential identity does not match the
    /// user id it was claimed to belong to.
    #[error(
        "Credential identity mismatch: expected '{expected}', key package credential is '{found}'"
    )]
    CredentialIdentityMismatch {
        /// The user id the key package was claimed to belong to.
        expected: String,
        /// The identity actually embedded in the key package credential.
        found: String,
    },
}
