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
//! and the original content type), and, since envelope v2, the rich message
//! extras (caption, reply threading, quoted-reply context, forward
//! attribution):
//!
//! ```text
//! [flags:1]
//! [flags & 0x01: meta_len:4 LE][MediaMetadata JSON]
//! [flags & 0x02: oct_len:1][original content type string]
//! [flags & 0x04: rich_len:4 LE][MediaRichExtras JSON]
//! [flags & 0x08: purpose_len:4 LE][DataPurpose JSON]
//! [chunk bytes = remainder]   (FileChunk::to_bytes)
//! ```
//!
//! Flag bits are NOT additively safe on their own — a decoder that ignored an
//! unknown bit would slurp the field's bytes as chunk data. Any chunk carrying
//! a flag beyond a receiver's known set must therefore ship under a bumped
//! envelope version (chunk 0 with rich extras ships as v2), so an old decoder
//! rejects the chunk cleanly at the version check instead of corrupting the
//! file; `decode` also rejects unknown flag bits outright as backstop.

use offline_protocol_core::{ContentType, ForwardInfo, MediaMetadata, ReplyContext};
use offline_protocol_mls::EncryptedMessage;
use serde::{Deserialize, Serialize};

/// Magic prefix identifying an encrypted media envelope in `binary_content`.
pub(crate) const MEDIA_ENVELOPE_MAGIC: [u8; 2] = *b"ML";

/// Envelope version 1: payload is an MLS [`EncryptedMessage`] in its compact
/// binary encoding.
pub(crate) const MEDIA_ENVELOPE_VERSION_MLS_V1: u8 = 0x01;

/// Envelope version 2: same payload as v1, but the decrypted plaintext may
/// carry the rich-extras field (`FLAG_RICH_EXTRAS`). Emitted only on chunk 0
/// and only toward recipients that advertised `RICH_PAYLOAD_V1` in their key
/// package, so a v1-only receiver never sees it; if a gating bug ever sends it
/// anyway, the old decoder rejects the unknown version instead of misparsing.
pub(crate) const MEDIA_ENVELOPE_VERSION_MLS_V2: u8 = 0x02;

/// Envelope version 3: the decrypted plaintext may carry the data-purpose
/// field (`FLAG_DATA_PURPOSE`), which says this transfer belongs to the
/// replicated-document layer rather than to the person using the app.
///
/// Emitted only on chunk 0 and only toward recipients that advertised
/// `DATA_MEDIA_V1` in their key package. The gate matters more here than on
/// v2: a receiver that does not understand the purpose does not merely lose
/// a caption, it hands a CRDT snapshot to its user as a downloaded file.
/// The version is the second line of that defence, and it is the one that
/// still holds if the capability gate is ever wrong.
pub(crate) const MEDIA_ENVELOPE_VERSION_MLS_V3: u8 = 0x03;

const FLAG_MEDIA_METADATA: u8 = 0x01;
const FLAG_ORIGINAL_CONTENT_TYPE: u8 = 0x02;
const FLAG_RICH_EXTRAS: u8 = 0x04;
const FLAG_DATA_PURPOSE: u8 = 0x08;

const KNOWN_FLAGS: u8 =
    FLAG_MEDIA_METADATA | FLAG_ORIGINAL_CONTENT_TYPE | FLAG_RICH_EXTRAS | FLAG_DATA_PURPOSE;

/// MediaMetadata JSON is small (thumbnail ≤ 2 KB base64); 256 KB is generous.
/// The plaintext is authenticated by MLS before parsing, so this is
/// belt-and-suspenders against a malicious group member.
const MAX_METADATA_JSON_LEN: usize = 256 * 1024;

/// Returns whether `data` carries the encrypted media envelope magic.
pub(crate) fn is_media_envelope(data: &[u8]) -> bool {
    data.len() > 2 && data[0..2] == MEDIA_ENVELOPE_MAGIC
}

/// Wraps an encrypted chunk in the versioned media envelope. `version` is
/// whatever [`MediaChunkPlaintext::envelope_version`] decided the plaintext
/// needs: the highest version any field present on it requires, so that a
/// receiver refuses at the version check rather than misreading a field it
/// does not know.
pub(crate) fn encode_media_envelope(encrypted: &EncryptedMessage, version: u8) -> Vec<u8> {
    let payload = encrypted.to_bytes();
    let mut buf = Vec::with_capacity(3 + payload.len());
    buf.extend_from_slice(&MEDIA_ENVELOPE_MAGIC);
    buf.push(version);
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
    if version != MEDIA_ENVELOPE_VERSION_MLS_V1
        && version != MEDIA_ENVELOPE_VERSION_MLS_V2
        && version != MEDIA_ENVELOPE_VERSION_MLS_V3
    {
        return Err(format!("unsupported media envelope version {}", version));
    }
    EncryptedMessage::from_bytes(&data[3..]).map_err(|e| e.to_string())
}

/// Rich message extras sealed on chunk 0 of a media transfer: caption, reply
/// threading, quoted-reply context, and forward attribution. Only ever
/// encoded toward recipients that advertised `RICH_PAYLOAD_V1` (under a v2
/// envelope) and never carried on the wire `Message` — plaintext (opt-out)
/// transfers drop them entirely. Additive fields go straight into this
/// struct (serde-default), needing no new flag bit or envelope version.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MediaRichExtras {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_to_msg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply_context: Option<ReplyContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) forward_info: Option<ForwardInfo>,
}

impl MediaRichExtras {
    /// Whether any rich field is present (empty extras never encode).
    pub(crate) fn is_any(&self) -> bool {
        self.caption.is_some()
            || self.reply_to_msg.is_some()
            || self.reply_context.is_some()
            || self.forward_info.is_some()
    }
}

/// Manual Debug: caption and quoted text are message content and must not
/// end up in logs by accident (same rule as `Event`'s redacting Debug).
impl std::fmt::Debug for MediaRichExtras {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaRichExtras")
            .field("caption", &self.caption.is_some())
            .field("reply_to_msg", &"[REDACTED]")
            .field("reply_context", &self.reply_context.is_some())
            .field("forward_info", &self.forward_info.is_some())
            .finish()
    }
}

/// Why a media transfer exists, when it exists for the replicated-document
/// layer rather than for the person using the app.
///
/// Present only on transfers this SDK starts on its own behalf. There is no
/// public API that sets it: an application asking to send a file cannot
/// produce one, which is the point. A transfer marked this way is routed
/// into the data layer on arrival and never surfaced as a received file, so
/// an app able to set it could feed bytes of its choosing to a peer's
/// document engine while the peer's user saw nothing.
///
/// The space is deliberately absent. On a 1:1 session the space is the
/// authenticated wire sender, exactly as it is for a sync frame, and a space
/// a peer could name is a space a peer could reach.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "p", rename_all = "snake_case")]
pub(crate) enum DataPurpose {
    /// The bytes of an attachment this peer asked for by hash.
    ///
    /// The hash is what the requester asked for, and the receiver checks the
    /// arriving bytes against it rather than against anything the sender
    /// says about them.
    Attachment {
        /// Lowercase hex SHA-256 the requester asked for.
        hash: String,
    },
    /// A whole document, for a replica whose gap no sync frame can close.
    ///
    /// The rung above `snap` on the catch-up ladder, reached when the
    /// document does not fit in a frame. Terminal in the same way: it
    /// provokes no answer.
    Snapshot {
        /// Document the bytes belong to, inside the derived space.
        doc: String,
    },
}

/// The authenticated plaintext of an encrypted media chunk.
pub(crate) struct MediaChunkPlaintext {
    /// `FileChunk::to_bytes()` of the carried chunk.
    pub chunk_bytes: Vec<u8>,

    /// Media metadata, present on chunk 0 only. Carried inside the ciphertext
    /// because it includes the file name and a preview thumbnail.
    pub media_metadata: Option<MediaMetadata>,

    /// Original content type (image, video, ...), present on chunk 0 only.
    pub original_content_type: Option<ContentType>,

    /// Rich message extras, present on chunk 0 only and only toward
    /// rich-capable recipients. Forces the v2 envelope when set.
    pub rich_extras: Option<MediaRichExtras>,

    /// Why this transfer exists, when it exists for the data layer. Present
    /// on chunk 0 only and only toward recipients that advertised
    /// `DATA_MEDIA_V1`. Forces the v3 envelope when set.
    pub data_purpose: Option<DataPurpose>,
}

/// Manual Debug: elides chunk bytes and delegates to the field types'
/// redacting Debug impls for anything content-bearing.
impl std::fmt::Debug for MediaChunkPlaintext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaChunkPlaintext")
            .field(
                "chunk_bytes",
                &format!("[{} bytes]", self.chunk_bytes.len()),
            )
            .field("media_metadata", &self.media_metadata.is_some())
            .field("original_content_type", &self.original_content_type)
            .field("rich_extras", &self.rich_extras)
            .field("data_purpose", &self.data_purpose)
            .finish()
    }
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
        let rich_json = match &self.rich_extras {
            Some(rich) => Some(
                serde_json::to_vec(rich)
                    .map_err(|e| format!("failed to serialize rich extras: {}", e))?,
            ),
            None => None,
        };
        if let Some(rich) = &rich_json {
            if rich.len() > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "rich extras length {} exceeds maximum {}",
                    rich.len(),
                    MAX_METADATA_JSON_LEN
                ));
            }
        }

        let purpose_json = match &self.data_purpose {
            Some(purpose) => Some(
                serde_json::to_vec(purpose)
                    .map_err(|e| format!("failed to serialize data purpose: {}", e))?,
            ),
            None => None,
        };
        if let Some(purpose) = &purpose_json {
            if purpose.len() > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "data purpose length {} exceeds maximum {}",
                    purpose.len(),
                    MAX_METADATA_JSON_LEN
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
        if rich_json.is_some() {
            flags |= FLAG_RICH_EXTRAS;
        }
        if purpose_json.is_some() {
            flags |= FLAG_DATA_PURPOSE;
        }

        let mut buf = Vec::with_capacity(
            1 + meta_json.as_ref().map_or(0, |m| 4 + m.len())
                + oct.as_ref().map_or(0, |s| 1 + s.len())
                + rich_json.as_ref().map_or(0, |r| 4 + r.len())
                + purpose_json.as_ref().map_or(0, |p| 4 + p.len())
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
        if let Some(rich) = rich_json {
            buf.extend_from_slice(&(rich.len() as u32).to_le_bytes());
            buf.extend_from_slice(&rich);
        }
        // Appended after every field that existed before it, because the
        // fields are positional and a decoder walks them in this order.
        if let Some(purpose) = purpose_json {
            buf.extend_from_slice(&(purpose.len() as u32).to_le_bytes());
            buf.extend_from_slice(&purpose);
        }
        buf.extend_from_slice(&self.chunk_bytes);
        Ok(buf)
    }

    /// The envelope version this plaintext must ship under: v3 when it
    /// carries a data purpose, v2 when it carries rich extras, v1 otherwise.
    ///
    /// Highest wins, and the ordering is what makes the rule safe: a
    /// plaintext carrying both fields must ship under the version that
    /// covers the later one, or a v2 receiver would accept the envelope and
    /// then read the purpose length as chunk bytes.
    pub(crate) fn envelope_version(&self) -> u8 {
        if self.data_purpose.is_some() {
            MEDIA_ENVELOPE_VERSION_MLS_V3
        } else if self.rich_extras.is_some() {
            MEDIA_ENVELOPE_VERSION_MLS_V2
        } else {
            MEDIA_ENVELOPE_VERSION_MLS_V1
        }
    }

    /// Parses a decrypted media chunk plaintext.
    pub(crate) fn decode(data: &[u8]) -> Result<Self, String> {
        if data.is_empty() {
            return Err("empty media chunk plaintext".to_string());
        }
        let flags = data[0];
        // A flag we don't know means a field we can't skip — everything after
        // the known fields would be misread as chunk bytes. Fail clean; the
        // sender should have bumped the envelope version for such a chunk.
        if flags & !KNOWN_FLAGS != 0 {
            return Err(format!("unknown media chunk flags {:#04x}", flags));
        }
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

        let rich_extras = if flags & FLAG_RICH_EXTRAS != 0 {
            if pos + 4 > data.len() {
                return Err("unexpected end of data reading rich extras length".to_string());
            }
            let mut length_bytes = [0u8; 4];
            length_bytes.copy_from_slice(&data[pos..pos + 4]);
            let rich_len = u32::from_le_bytes(length_bytes) as usize;
            pos += 4;
            if rich_len > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "rich extras length {} exceeds maximum {}",
                    rich_len, MAX_METADATA_JSON_LEN
                ));
            }
            if pos + rich_len > data.len() {
                return Err("unexpected end of data reading rich extras".to_string());
            }
            let rich: MediaRichExtras = serde_json::from_slice(&data[pos..pos + rich_len])
                .map_err(|e| format!("invalid rich extras JSON: {}", e))?;
            pos += rich_len;
            Some(rich)
        } else {
            None
        };

        let data_purpose = if flags & FLAG_DATA_PURPOSE != 0 {
            if pos + 4 > data.len() {
                return Err("unexpected end of data reading data purpose length".to_string());
            }
            let mut length_bytes = [0u8; 4];
            length_bytes.copy_from_slice(&data[pos..pos + 4]);
            let purpose_len = u32::from_le_bytes(length_bytes) as usize;
            pos += 4;
            if purpose_len > MAX_METADATA_JSON_LEN {
                return Err(format!(
                    "data purpose length {} exceeds maximum {}",
                    purpose_len, MAX_METADATA_JSON_LEN
                ));
            }
            if pos + purpose_len > data.len() {
                return Err("unexpected end of data reading data purpose".to_string());
            }
            let purpose: DataPurpose = serde_json::from_slice(&data[pos..pos + purpose_len])
                .map_err(|e| format!("invalid data purpose JSON: {}", e))?;
            pos += purpose_len;
            Some(purpose)
        } else {
            None
        };

        Ok(Self {
            chunk_bytes: data[pos..].to_vec(),
            media_metadata,
            original_content_type,
            rich_extras,
            data_purpose,
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
            media_id: None,
            download_url: None,
            thumbnail_url: None,
            encryption_key: None,
            iv: None,
            ciphertext_hash: None,
            sticker_provider: None,
            sticker_remote_id: None,
            sticker_kind: None,
        }
    }

    fn sample_rich_extras() -> MediaRichExtras {
        use offline_protocol_core::UserId;
        MediaRichExtras {
            caption: Some("look at this".to_string()),
            reply_to_msg: Some("0192aaaa-bbbb-cccc-dddd-eeeeffff0000".to_string()),
            reply_context: Some(ReplyContext {
                sender: UserId::new("carol").unwrap(),
                text: "the original".to_string(),
                timestamp: None,
                reply_media_label: None,
                reply_content_type: Some("text".to_string()),
            }),
            forward_info: None,
        }
    }

    #[test]
    fn test_envelope_roundtrip() {
        let encrypted = sample_encrypted();
        for version in [MEDIA_ENVELOPE_VERSION_MLS_V1, MEDIA_ENVELOPE_VERSION_MLS_V2] {
            let envelope = encode_media_envelope(&encrypted, version);

            assert!(is_media_envelope(&envelope));
            assert_eq!(envelope[2], version);
            let decoded = decode_media_envelope(&envelope).unwrap();
            assert_eq!(decoded.ciphertext, encrypted.ciphertext);
            assert_eq!(decoded.group_id, encrypted.group_id);
            assert_eq!(decoded.sender_id, encrypted.sender_id);
        }
    }

    #[test]
    fn test_envelope_unknown_version_rejected() {
        let mut envelope =
            encode_media_envelope(&sample_encrypted(), MEDIA_ENVELOPE_VERSION_MLS_V1);
        envelope[2] = 0x7F;
        assert!(decode_media_envelope(&envelope)
            .unwrap_err()
            .contains("version"));
    }

    #[test]
    fn test_v2_envelope_rejected_by_v1_only_version_check() {
        // What a pre-rich receiver does with a v2 envelope: its decoder only
        // accepts 0x01, so the chunk dies at the version check (clean drop)
        // rather than having the rich field slurped into the file bytes.
        let envelope = encode_media_envelope(&sample_encrypted(), MEDIA_ENVELOPE_VERSION_MLS_V2);
        assert_ne!(envelope[2], MEDIA_ENVELOPE_VERSION_MLS_V1);
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
            rich_extras: None,
            data_purpose: None,
        };
        assert_eq!(plain.envelope_version(), MEDIA_ENVELOPE_VERSION_MLS_V1);
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();
        assert_eq!(decoded.chunk_bytes, vec![7; 64]);
        assert!(decoded.media_metadata.is_none());
        assert!(decoded.original_content_type.is_none());
        assert!(decoded.rich_extras.is_none());
    }

    #[test]
    fn test_plaintext_roundtrip_with_metadata_and_content_type() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![42; 16],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Image),
            rich_extras: None,
            data_purpose: None,
        };
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();

        assert_eq!(decoded.chunk_bytes, vec![42; 16]);
        let meta = decoded.media_metadata.unwrap();
        assert_eq!(meta.file_name, "photo.jpg");
        assert_eq!(meta.thumbnail_base64.as_deref(), Some("dGh1bWI="));
        assert_eq!(decoded.original_content_type, Some(ContentType::Image));
        assert!(decoded.rich_extras.is_none());
    }

    #[test]
    fn test_plaintext_roundtrip_with_rich_extras() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![42; 16],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Image),
            rich_extras: Some(sample_rich_extras()),
            data_purpose: None,
        };
        assert_eq!(plain.envelope_version(), MEDIA_ENVELOPE_VERSION_MLS_V2);
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();

        assert_eq!(decoded.chunk_bytes, vec![42; 16]);
        assert_eq!(decoded.original_content_type, Some(ContentType::Image));
        let rich = decoded.rich_extras.unwrap();
        assert_eq!(rich, sample_rich_extras());
    }

    #[test]
    fn test_plaintext_roundtrip_with_data_purpose() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![42; 16],
            media_metadata: None,
            original_content_type: Some(ContentType::File),
            rich_extras: None,
            data_purpose: Some(DataPurpose::Attachment {
                hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
            }),
        };
        assert_eq!(plain.envelope_version(), MEDIA_ENVELOPE_VERSION_MLS_V3);
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();
        assert_eq!(decoded.chunk_bytes, vec![42; 16]);
        assert_eq!(decoded.data_purpose, plain.data_purpose);
    }

    #[test]
    fn test_data_purpose_forces_the_v3_envelope_even_beside_rich_extras() {
        // The flag fields are positional, so a decoder walks them in order
        // and stops knowing where it is the moment it meets one it cannot
        // skip. A plaintext carrying both fields must therefore ship under
        // the version covering the LATER one: under v2 a receiver would
        // accept the envelope and then read the purpose length as chunk
        // bytes, corrupting the file rather than refusing it.
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![7; 4],
            media_metadata: None,
            original_content_type: None,
            rich_extras: Some(sample_rich_extras()),
            data_purpose: Some(DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        };
        assert_eq!(plain.envelope_version(), MEDIA_ENVELOPE_VERSION_MLS_V3);
        let decoded = MediaChunkPlaintext::decode(&plain.encode().unwrap()).unwrap();
        assert_eq!(decoded.rich_extras, plain.rich_extras);
        assert_eq!(decoded.data_purpose, plain.data_purpose);
        assert_eq!(decoded.chunk_bytes, vec![7; 4]);
    }

    #[test]
    fn test_a_v2_decoder_refuses_a_purpose_it_cannot_skip() {
        // What a receiver predating this field does with one, and why the
        // version bump above is the second line of defence rather than the
        // first: the unknown flag is refused outright, so the chunk is
        // dropped instead of being misread.
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1; 4],
            media_metadata: None,
            original_content_type: None,
            rich_extras: None,
            data_purpose: Some(DataPurpose::Snapshot {
                doc: "notes".to_string(),
            }),
        };
        let bytes = plain.encode().unwrap();
        assert_eq!(bytes[0] & FLAG_DATA_PURPOSE, FLAG_DATA_PURPOSE);

        // Stand in for the older decoder by masking the flag out of its
        // known set, which is exactly what that build's `KNOWN_FLAGS` did.
        let older_known = FLAG_MEDIA_METADATA | FLAG_ORIGINAL_CONTENT_TYPE | FLAG_RICH_EXTRAS;
        assert_ne!(
            bytes[0] & !older_known,
            0,
            "an older decoder must see a flag it does not know, and refuse"
        );
    }

    #[test]
    fn test_plaintext_without_a_data_purpose_is_byte_identical_to_before() {
        // Pay for what you use: a chunk with no purpose encodes exactly as
        // it did before the field existed, so every shipped transfer is
        // unchanged on the wire.
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![9; 8],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Image),
            rich_extras: None,
            data_purpose: None,
        };
        let bytes = plain.encode().unwrap();
        assert_eq!(bytes[0] & FLAG_DATA_PURPOSE, 0);
        assert_eq!(plain.envelope_version(), MEDIA_ENVELOPE_VERSION_MLS_V1);
    }

    #[test]
    fn test_plaintext_without_rich_extras_is_byte_identical_to_v1_encoding() {
        // The rich field must be pay-for-what-you-use: a no-extras chunk
        // encodes exactly as it did before the field existed.
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![9; 8],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Image),
            rich_extras: None,
            data_purpose: None,
        };
        let bytes = plain.encode().unwrap();
        assert_eq!(bytes[0] & FLAG_RICH_EXTRAS, 0);
        assert_eq!(bytes[0], FLAG_MEDIA_METADATA | FLAG_ORIGINAL_CONTENT_TYPE);
    }

    #[test]
    fn test_plaintext_decode_unknown_flag_rejected() {
        // An unknown flag bit means an unskippable field: fail clean instead
        // of slurping it as chunk bytes.
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1, 2, 3],
            media_metadata: None,
            original_content_type: None,
            rich_extras: None,
            data_purpose: None,
        };
        let mut bytes = plain.encode().unwrap();
        bytes[0] |= 0x40;
        assert!(MediaChunkPlaintext::decode(&bytes)
            .unwrap_err()
            .contains("unknown media chunk flags"));
    }

    #[test]
    fn test_plaintext_decode_malformed_rich_extras_rejected() {
        // Rich extras are MLS-authenticated, so malformed JSON is a sender
        // bug or malice — drop the chunk like malformed metadata, don't
        // guess.
        let garbage = b"not json";
        let mut bytes = vec![FLAG_RICH_EXTRAS];
        bytes.extend_from_slice(&(garbage.len() as u32).to_le_bytes());
        bytes.extend_from_slice(garbage);
        assert!(MediaChunkPlaintext::decode(&bytes)
            .unwrap_err()
            .contains("invalid rich extras JSON"));
    }

    #[test]
    fn test_plaintext_decode_oversized_rich_extras_rejected() {
        let mut bytes = vec![FLAG_RICH_EXTRAS];
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(MediaChunkPlaintext::decode(&bytes)
            .unwrap_err()
            .contains("rich extras length"));
    }

    #[test]
    fn test_plaintext_decode_truncated() {
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1, 2, 3],
            media_metadata: Some(sample_metadata()),
            original_content_type: Some(ContentType::Video),
            rich_extras: Some(sample_rich_extras()),
            data_purpose: None,
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
            rich_extras: None,
            data_purpose: None,
        };
        assert!(plain
            .encode()
            .unwrap_err()
            .contains("media metadata length"));
    }

    #[test]
    fn test_plaintext_encode_oversized_rich_extras_rejected() {
        let mut rich = sample_rich_extras();
        rich.caption = Some("A".repeat(MAX_METADATA_JSON_LEN + 1));
        let plain = MediaChunkPlaintext {
            chunk_bytes: vec![1, 2, 3],
            media_metadata: None,
            original_content_type: None,
            rich_extras: Some(rich),
            data_purpose: None,
        };
        assert!(plain.encode().unwrap_err().contains("rich extras length"));
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
