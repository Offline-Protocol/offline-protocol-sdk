//! Wire codec for MLS-encrypted media chunks (SEC-H1).
//!
//! File chunks ride in `Message.binary_content`. Encrypted chunks are wrapped
//! in a versioned envelope so receivers can distinguish them from legacy
//! plaintext chunks and apply policy (reject unencrypted media):
//!
//! ```text
//! [magic:2 = "ML"][version:1 = 0x01][EncryptedMessage compact bytes]
//! ```
//!
//! Legacy plaintext chunks are raw `FileChunk::to_bytes()`, which start with a
//! `file_id` length (`u32` LE) capped at 4096 — their second byte is the high
//! byte of that length (≤ 0x10) and can never equal the second magic byte
//! (0x4C), so the two formats are unambiguous.
//!
//! The plaintext that the envelope's ciphertext decrypts to carries the chunk
//! bytes plus the fields that previously leaked in cleartext on the chunk-0
//! `Message` (media metadata — including file name and preview thumbnail —
//! and the original content type):
//!
//! ```text
//! [flags:1]
//! [flags & 0x01: meta_len:4 LE][MediaMetadata JSON]
//! [flags & 0x02: oct_len:1][original content type string]
//! [chunk bytes = remainder]   (FileChunk::to_bytes)
//! ```

use offline_protocol_core::{ContentType, MediaMetadata};
use offline_protocol_mls::EncryptedMessage;

/// Magic prefix identifying an encrypted media envelope in `binary_content`.
pub(crate) const MEDIA_ENVELOPE_MAGIC: [u8; 2] = *b"ML";

/// Envelope version 1: payload is an MLS [`EncryptedMessage`] in its compact
/// binary encoding.
pub(crate) const MEDIA_ENVELOPE_VERSION_MLS_V1: u8 = 0x01;

const FLAG_MEDIA_METADATA: u8 = 0x01;
const FLAG_ORIGINAL_CONTENT_TYPE: u8 = 0x02;

/// MediaMetadata JSON is small (thumbnail ≤ 2 KB base64); 256 KB is generous.
/// The plaintext is authenticated by MLS before parsing, so this is
/// belt-and-suspenders against a malicious group member.
const MAX_METADATA_JSON_LEN: usize = 256 * 1024;

/// Returns whether `data` carries the encrypted media envelope magic.
pub(crate) fn is_media_envelope(data: &[u8]) -> bool {
    data.len() > 2 && data[0..2] == MEDIA_ENVELOPE_MAGIC
}

/// Wraps an encrypted chunk in the versioned media envelope.
pub(crate) fn encode_media_envelope(encrypted: &EncryptedMessage) -> Vec<u8> {
    let payload = encrypted.to_bytes();
    let mut buf = Vec::with_capacity(3 + payload.len());
    buf.extend_from_slice(&MEDIA_ENVELOPE_MAGIC);
    buf.push(MEDIA_ENVELOPE_VERSION_MLS_V1);
    buf.extend_from_slice(&payload);
    buf
}

/// Unwraps a media envelope produced by [`encode_media_envelope`].
///
/// The caller must have checked [`is_media_envelope`] first; an unknown
/// version (from a newer peer) is an error so the chunk is dropped rather
/// than misparsed.
pub(crate) fn decode_media_envelope(data: &[u8]) -> Result<EncryptedMessage, String> {
    if !is_media_envelope(data) {
        return Err("missing media envelope magic".to_string());
    }
    let version = data[2];
    if version != MEDIA_ENVELOPE_VERSION_MLS_V1 {
        return Err(format!("unsupported media envelope version {}", version));
    }
    EncryptedMessage::from_bytes(&data[3..]).map_err(|e| e.to_string())
}

/// The authenticated plaintext of an encrypted media chunk.
#[derive(Debug)]
pub(crate) struct MediaChunkPlaintext {
    /// `FileChunk::to_bytes()` of the carried chunk.
    pub chunk_bytes: Vec<u8>,

    /// Media metadata, present on chunk 0 only. Carried inside the ciphertext
    /// because it includes the file name and a preview thumbnail.
    pub media_metadata: Option<MediaMetadata>,

    /// Original content type (image, video, ...), present on chunk 0 only.
    pub original_content_type: Option<ContentType>,
}

impl MediaChunkPlaintext {
    /// Serializes the plaintext for encryption.
    ///
    /// Enforces the same field bounds `decode` does, so an oversized payload
    /// fails the send with a clear error instead of being silently dropped by
    /// the receiver.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        let meta_json = match &self.media_metadata {
            Some(meta) => Some(
                serde_json::to_vec(meta)
                    .map_err(|e| format!("failed to serialize media metadata: {}", e))?,
            ),
            None => None,
        };
        if let Some(meta) = &meta_json {
            if meta.len() > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "media metadata length {} exceeds maximum {} (thumbnail too large?)",
                    meta.len(),
                    MAX_METADATA_JSON_LEN
                ));
            }
        }
        let oct = self.original_content_type.as_ref().map(|ct| ct.to_string());
        if let Some(oct) = &oct {
            if oct.len() > u8::MAX as usize {
                return Err(format!(
                    "content type string length {} exceeds maximum {}",
                    oct.len(),
                    u8::MAX
                ));
            }
        }

        let mut flags = 0u8;
        if meta_json.is_some() {
            flags |= FLAG_MEDIA_METADATA;
        }
        if oct.is_some() {
            flags |= FLAG_ORIGINAL_CONTENT_TYPE;
        }

        let mut buf = Vec::with_capacity(
            1 + meta_json.as_ref().map_or(0, |m| 4 + m.len())
                + oct.as_ref().map_or(0, |s| 1 + s.len())
                + self.chunk_bytes.len(),
        );
        buf.push(flags);
        if let Some(meta) = meta_json {
            buf.extend_from_slice(&(meta.len() as u32).to_le_bytes());
            buf.extend_from_slice(&meta);
        }
        if let Some(oct) = oct {
            let bytes = oct.as_bytes();
            buf.push(bytes.len() as u8);
            buf.extend_from_slice(bytes);
        }
        buf.extend_from_slice(&self.chunk_bytes);
        Ok(buf)
    }

    /// Parses a decrypted media chunk plaintext.
    pub(crate) fn decode(data: &[u8]) -> Result<Self, String> {
        if data.is_empty() {
            return Err("empty media chunk plaintext".to_string());
        }
        let flags = data[0];
        let mut pos = 1;

        let media_metadata = if flags & FLAG_MEDIA_METADATA != 0 {
            if pos + 4 > data.len() {
                return Err("unexpected end of data reading metadata length".to_string());
            }
            let mut length_bytes = [0u8; 4];
            length_bytes.copy_from_slice(&data[pos..pos + 4]);
            let meta_len = u32::from_le_bytes(length_bytes) as usize;
            pos += 4;
            if meta_len > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "metadata length {} exceeds maximum {}",
                    meta_len, MAX_METADATA_JSON_LEN
                ));
            }
            if pos + meta_len > data.len() {
                return Err("unexpected end of data reading metadata".to_string());
            }
            let meta: MediaMetadata = serde_json::from_slice(&data[pos..pos + meta_len])
                .map_err(|e| format!("invalid media metadata JSON: {}", e))?;
            pos += meta_len;
            Some(meta)
        } else {
            None
        };

        let original_content_type = if flags & FLAG_ORIGINAL_CONTENT_TYPE != 0 {
            if pos + 1 > data.len() {
                return Err("unexpected end of data reading content type length".to_string());
            }
            let oct_len = data[pos] as usize;
            pos += 1;
            if pos + oct_len > data.len() {
                return Err("unexpected end of data reading content type".to_string());
            }
            let s = std::str::from_utf8(&data[pos..pos + oct_len])
                .map_err(|e| format!("invalid content type UTF-8: {}", e))?;
            pos += oct_len;
            Some(ContentType::parse(s))
        } else {
            None
        };

        Ok(Self {
            chunk_bytes: data[pos..].to_vec(),
            media_metadata,
            original_content_type,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use offline_protocol_mls::{GroupId, MlsMessageType};

    fn sample_encrypted() -> EncryptedMessage {
        EncryptedMessage {
            group_id: GroupId::for_session("alice", "bob").unwrap(),
            message_type: MlsMessageType::Application,
            epoch: 3,
            ciphertext: vec![1, 2, 3, 4, 5],
            sender_id: "alice".to_string(),
            timestamp_ms: 1_700_000_000_000,
        }
    }

    fn sample_metadata() -> MediaMetadata {
        MediaMetadata {
            mime_type: "image/jpeg".to_string(),
            file_name: "photo.jpg".to_string(),
            file_size: 12345,
            duration_ms: None,
            width: Some(800),
            height: Some(600),
            thumbnail_base64: Some("dGh1bWI=".to_string()),
        }
    }

    #[test]
    fn test_envelope_roundtrip() {
        let encrypted = sample_encrypted();
        let envelope = encode_media_envelope(&encrypted);

        assert!(is_media_envelope(&envelope));
        let decoded = decode_media_envelope(&envelope).unwrap();
        assert_eq!(decoded.ciphertext, encrypted.ciphertext);
        assert_eq!(decoded.group_id, encrypted.group_id);
        assert_eq!(decoded.sender_id, encrypted.sender_id);
    }

    #[test]
    fn test_envelope_unknown_version_rejected() {
        let mut envelope = encode_media_envelope(&sample_encrypted());
        envelope[2] = 0x7F;
        assert!(decode_media_envelope(&envelope)
            .unwrap_err()
            .contains("version"));
    }

    #[test]
    fn test_legacy_chunk_bytes_are_not_an_envelope() {
        // A raw FileChunk::to_bytes() payload starts with the file_id length
        // (u32 LE, capped at 4096) — byte 1 can never match the magic.
        let chunk = crate::file_transfer::FileChunk {
            file_id: "file_abc".to_string(),
            file_name: "f.bin".to_string(),
            file_size: 3,
            total_chunks: 1,
            chunk_index: 0,
            chunk_data: vec![9, 9, 9],
            file_checksum: "cafe".to_string(),
        };
        assert!(!is_media_envelope(&chunk.to_bytes()));
    }

    #[test]
    fn test_plaintext_roundtrip_chunk_only() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![7; 64],
            media_metadata: None,
            original_content_type: None,
        };
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();
        assert_eq!(decoded.chunk_bytes, vec![7; 64]);
        assert!(decoded.media_metadata.is_none());
        assert!(decoded.original_content_type.is_none());
    }

    #[test]
    fn test_plaintext_roundtrip_with_metadata_and_content_type() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![42; 16],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Image),
        };
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();

        assert_eq!(decoded.chunk_bytes, vec![42; 16]);
        let meta = decoded.media_metadata.unwrap();
        assert_eq!(meta.file_name, "photo.jpg");
        assert_eq!(meta.thumbnail_base64.as_deref(), Some("dGh1bWI="));
        assert_eq!(decoded.original_content_type, Some(ContentType::Image));
    }

    #[test]
    fn test_plaintext_decode_truncated() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1, 2, 3],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Video),
        };
        let bytes = plain.encode().unwrap();
        // Truncations inside the metadata/content-type headers must fail
        // cleanly; truncations inside the trailing chunk bytes are legal at
        // the codec layer (FileChunk::from_bytes catches them next).
        let chunk_start = bytes.len() - 3;
        for len in 0..chunk_start {
            assert!(
                MediaChunkPlaintext::decode(&bytes[..len]).is_err(),
                "truncation at {} unexpectedly succeeded",
                len
            );
        }
    }

    #[test]
    fn test_plaintext_encode_oversized_metadata_rejected() {
        // An oversized thumbnail must fail at the sender with a clear error,
        // not encode successfully and be dropped by the receiver's decode cap.
        let mut meta = sample_metadata();
        meta.thumbnail_base64 = Some("A".repeat(MAX_METADATA_JSON_LEN + 1));
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1, 2, 3],
            media_metadata: Some(meta),
            original_content_type: None,
        };
        assert!(plain
            .encode()
            .unwrap_err()
            .contains("media metadata length"));
    }

    #[test]
    fn test_plaintext_decode_oversized_metadata_rejected() {
        let mut bytes = vec![FLAG_MEDIA_METADATA];
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(MediaChunkPlaintext::decode(&bytes)
            .unwrap_err()
            .contains("metadata length"));
    }
}
