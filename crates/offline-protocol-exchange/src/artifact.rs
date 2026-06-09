//! Adapter artifact integrity: content-addressed verification.
//!
//! Adapter weights move over the file-transfer path; on arrival the bytes are
//! hashed and compared against the attested [`ArtifactRef`]. A mismatch
//! rejects the artifact — the runtime must never load unverified weights.

use crate::error::{ExchangeError, ExchangeResult};
use crate::types::ArtifactRef;
use sha2::{Digest, Sha256};

/// Computes the lowercase-hex SHA-256 of artifact bytes.
pub fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Verifies received artifact bytes against their attested reference.
///
/// Checks the exact size first (cheap) and then the content hash. Returns
/// `Err(ArtifactVerificationFailed)` on any mismatch.
pub fn verify_artifact(bytes: &[u8], artifact: &ArtifactRef) -> ExchangeResult<()> {
    if bytes.len() as u64 != artifact.size_bytes {
        return Err(ExchangeError::ArtifactVerificationFailed(format!(
            "size mismatch: got {} bytes, attested {}",
            bytes.len(),
            artifact.size_bytes
        )));
    }
    let actual = content_hash(bytes);
    if actual != artifact.content_hash {
        return Err(ExchangeError::ArtifactVerificationFailed(format!(
            "content hash mismatch: got {actual}, attested {}",
            artifact.content_hash
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChunkPlan;

    fn artifact_for(bytes: &[u8]) -> ArtifactRef {
        ArtifactRef {
            content_hash: content_hash(bytes),
            size_bytes: bytes.len() as u64,
            base_model: "gemma-3-1b".into(),
            base_model_version: "1.0".into(),
            chunking: ChunkPlan {
                chunk_size_bytes: 65536,
            },
        }
    }

    #[test]
    fn matching_artifact_verifies() {
        let bytes = b"adapter-weights-bytes";
        assert!(verify_artifact(bytes, &artifact_for(bytes)).is_ok());
    }

    #[test]
    fn flipped_byte_rejected() {
        let bytes = b"adapter-weights-bytes".to_vec();
        let artifact = artifact_for(&bytes);
        let mut tampered = bytes;
        tampered[0] ^= 0x01;
        assert!(matches!(
            verify_artifact(&tampered, &artifact),
            Err(ExchangeError::ArtifactVerificationFailed(_))
        ));
    }

    #[test]
    fn size_mismatch_rejected() {
        let bytes = b"adapter-weights-bytes";
        let mut artifact = artifact_for(bytes);
        artifact.size_bytes += 1;
        assert!(matches!(
            verify_artifact(bytes, &artifact),
            Err(ExchangeError::ArtifactVerificationFailed(_))
        ));
    }

    #[test]
    fn known_hash_value() {
        // SHA-256 of empty input — pin the encoding (lowercase hex).
        assert_eq!(
            content_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
