//! File transfer with chunking and reassembly.

use crate::constants::DEFAULT_CHUNK_SIZE;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

/// File transfer configuration.
#[derive(Debug, Clone)]
pub struct FileTransferConfig {
    /// Size of each chunk in bytes.
    pub chunk_size: usize,

    /// Maximum file size allowed (bytes).
    pub max_file_size: u64,

    /// Maximum number of inbound files that may be reassembling at once.
    /// Chunks that would start an additional assembly are rejected; slots
    /// free when a transfer completes, is cancelled, or goes stale.
    pub max_concurrent_assemblies: usize,

    /// Maximum number of inbound files a single sender may have reassembling
    /// at once, so one peer cannot occupy every assembly slot.
    pub max_assemblies_per_sender: usize,

    /// Maximum total bytes buffered across all inbound assemblies. This —
    /// not the assembly counts — is the hard bound on receive-path memory.
    /// Each stored chunk is charged its payload plus a small fixed
    /// bookkeeping overhead, so floods of tiny chunks cannot pin more
    /// memory than the budget admits. Values below what a single
    /// `max_file_size` transfer charges are clamped up to it by
    /// [`FileTransferManager::with_config`], or a maximum-size file could
    /// never complete.
    pub max_total_buffered_bytes: u64,
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_file_size: 100 * 1024 * 1024, // 100 MB
            max_concurrent_assemblies: 32,
            max_assemblies_per_sender: 16,
            max_total_buffered_bytes: 128 * 1024 * 1024, // 128 MB
        }
    }
}

/// Why [`FileTransferManager::process_chunk`] refused a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkRejection {
    /// Wire fields are malformed or out of bounds: zero or oversized size
    /// claims, empty or oversized data, or bytes exceeding the claimed
    /// file size.
    Invalid,
    /// Chunk metadata does not match the existing assembly for this file id.
    MetadataMismatch,
    /// Chunk sender does not match the sender that started the assembly.
    SenderMismatch,
    /// The global concurrent-assembly cap is reached.
    TooManyTransfers,
    /// The sender's concurrent-assembly quota is reached.
    SenderQuotaExceeded,
    /// Accepting the chunk would exceed the global buffered-bytes budget.
    /// The affected assembly is dropped along with the chunk: a dropped
    /// chunk is never retransmitted, so the transfer could no longer
    /// complete and its buffer would only prolong the exhaustion.
    BufferBudgetExhausted,
}

impl ChunkRejection {
    /// `true` for rejections caused by receiver resource limits rather than
    /// malformed input — the cases where a well-formed transfer was lost and
    /// the application should be told.
    pub fn is_resource_exhaustion(self) -> bool {
        matches!(
            self,
            Self::TooManyTransfers | Self::SenderQuotaExceeded | Self::BufferBudgetExhausted
        )
    }

    /// Stable machine-readable reason string, carried by the
    /// `FileReceiveFailed` event.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_chunk",
            Self::MetadataMismatch => "metadata_mismatch",
            Self::SenderMismatch => "sender_mismatch",
            Self::TooManyTransfers => "too_many_transfers",
            Self::SenderQuotaExceeded => "sender_quota_exceeded",
            Self::BufferBudgetExhausted => "buffer_budget_exhausted",
        }
    }
}

/// A file chunk message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    /// Unique file identifier.
    pub file_id: String,

    /// Original file name.
    pub file_name: String,

    /// Total file size in bytes.
    pub file_size: u64,

    /// Total number of chunks.
    pub total_chunks: u32,

    /// Index of this chunk (0-based).
    pub chunk_index: u32,

    /// Chunk data (base64-encoded in JSON).
    #[serde(with = "base64_bytes")]
    pub chunk_data: Vec<u8>,

    /// Checksum of the complete file (SHA256 hex string).
    pub file_checksum: String,
}

impl FileChunk {
    /// Serializes to JSON (kept for backward compatibility).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserializes from JSON (kept for backward compatibility).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serializes to a compact binary format, avoiding base64/JSON overhead.
    ///
    /// Wire format (all multi-byte integers are little-endian):
    /// ```text
    /// [file_id_len:4][file_id][file_name_len:4][file_name]
    /// [file_size:8][total_chunks:4][chunk_index:4]
    /// [chunk_data_len:4][chunk_data]
    /// [checksum_len:4][checksum]
    /// ```
    pub fn to_bytes(&self) -> Vec<u8> {
        let file_id = self.file_id.as_bytes();
        let file_name = self.file_name.as_bytes();
        let checksum = self.file_checksum.as_bytes();

        let capacity = 4
            + file_id.len()
            + 4
            + file_name.len()
            + 8
            + 4
            + 4
            + 4
            + self.chunk_data.len()
            + 4
            + checksum.len();
        let mut buf = Vec::with_capacity(capacity);

        buf.extend_from_slice(&(file_id.len() as u32).to_le_bytes());
        buf.extend_from_slice(file_id);
        buf.extend_from_slice(&(file_name.len() as u32).to_le_bytes());
        buf.extend_from_slice(file_name);
        buf.extend_from_slice(&self.file_size.to_le_bytes());
        buf.extend_from_slice(&self.total_chunks.to_le_bytes());
        buf.extend_from_slice(&self.chunk_index.to_le_bytes());
        buf.extend_from_slice(&(self.chunk_data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.chunk_data);
        buf.extend_from_slice(&(checksum.len() as u32).to_le_bytes());
        buf.extend_from_slice(checksum);

        buf
    }

    /// Maximum allowed length for a string field (file_id, file_name, checksum)
    /// in the binary wire format. Prevents allocation bombs from crafted payloads.
    const MAX_STRING_FIELD_LEN: usize = 4096;
    /// Maximum allowed chunk data length in the binary wire format.
    const MAX_CHUNK_DATA_LEN: usize = 128 * 1024 * 1024; // 128 MB

    /// Deserializes from the compact binary format produced by `to_bytes`.
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let mut pos = 0;

        let read_u32 = |pos: &mut usize| -> Result<u32, String> {
            if *pos + 4 > data.len() {
                return Err("unexpected end of data reading u32".to_string());
            }
            let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
            *pos += 4;
            Ok(val)
        };

        let read_u64 = |pos: &mut usize| -> Result<u64, String> {
            if *pos + 8 > data.len() {
                return Err("unexpected end of data reading u64".to_string());
            }
            let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(val)
        };

        let read_bytes = |pos: &mut usize, len: usize| -> Result<Vec<u8>, String> {
            if *pos + len > data.len() {
                return Err("unexpected end of data reading bytes".to_string());
            }
            let val = data[*pos..*pos + len].to_vec();
            *pos += len;
            Ok(val)
        };

        let file_id_len = read_u32(&mut pos)? as usize;
        if file_id_len > Self::MAX_STRING_FIELD_LEN {
            return Err(format!(
                "file_id_len {} exceeds maximum {}",
                file_id_len,
                Self::MAX_STRING_FIELD_LEN
            ));
        }
        let file_id = String::from_utf8(read_bytes(&mut pos, file_id_len)?)
            .map_err(|e| format!("invalid file_id UTF-8: {}", e))?;

        let file_name_len = read_u32(&mut pos)? as usize;
        if file_name_len > Self::MAX_STRING_FIELD_LEN {
            return Err(format!(
                "file_name_len {} exceeds maximum {}",
                file_name_len,
                Self::MAX_STRING_FIELD_LEN
            ));
        }
        let file_name = String::from_utf8(read_bytes(&mut pos, file_name_len)?)
            .map_err(|e| format!("invalid file_name UTF-8: {}", e))?;

        let file_size = read_u64(&mut pos)?;
        let total_chunks = read_u32(&mut pos)?;
        let chunk_index = read_u32(&mut pos)?;

        let chunk_data_len = read_u32(&mut pos)? as usize;
        if chunk_data_len > Self::MAX_CHUNK_DATA_LEN {
            return Err(format!(
                "chunk_data_len {} exceeds maximum {}",
                chunk_data_len,
                Self::MAX_CHUNK_DATA_LEN
            ));
        }
        let chunk_data = read_bytes(&mut pos, chunk_data_len)?;

        let checksum_len = read_u32(&mut pos)? as usize;
        if checksum_len > Self::MAX_STRING_FIELD_LEN {
            return Err(format!(
                "checksum_len {} exceeds maximum {}",
                checksum_len,
                Self::MAX_STRING_FIELD_LEN
            ));
        }
        let file_checksum = String::from_utf8(read_bytes(&mut pos, checksum_len)?)
            .map_err(|e| format!("invalid checksum UTF-8: {}", e))?;

        Ok(Self {
            file_id,
            file_name,
            file_size,
            total_chunks,
            chunk_index,
            chunk_data,
            file_checksum,
        })
    }
}

/// File transfer progress information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileProgress {
    /// File identifier.
    pub file_id: String,

    /// File name.
    pub file_name: String,

    /// Total file size.
    pub file_size: u64,

    /// Number of chunks sent/received.
    pub chunks_completed: u32,

    /// Total number of chunks.
    pub total_chunks: u32,

    /// Progress percentage (0-100).
    pub percentage: u8,
}

impl FileProgress {
    /// Calculates the progress percentage.
    pub fn calculate_percentage(chunks_completed: u32, total_chunks: u32) -> u8 {
        if total_chunks == 0 {
            return 0;
        }
        ((chunks_completed as f32 / total_chunks as f32) * 100.0) as u8
    }
}

/// An incomplete transfer removed by
/// [`FileTransferManager::cleanup_stale_transfers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleTransfer {
    /// File identifier of the dropped transfer.
    pub file_id: String,
    /// File name from the chunk metadata.
    pub file_name: String,
    /// Sender the assembly was bound to.
    pub sender: String,
}

/// Tracks file reassembly state.
struct FileAssembly {
    /// Sender the first accepted chunk arrived from; all later chunks must
    /// match it.
    sender: String,
    file_name: String,
    file_size: u64,
    total_chunks: u32,
    file_checksum: String,
    received_chunks: HashMap<u32, Vec<u8>>,
    /// Total bytes currently held in `received_chunks`.
    received_bytes: u64,
    last_updated_at: Instant,
}

impl FileAssembly {
    fn new(sender: &str, chunk: &FileChunk) -> Self {
        Self {
            sender: sender.to_string(),
            file_name: chunk.file_name.clone(),
            file_size: chunk.file_size,
            total_chunks: chunk.total_chunks,
            file_checksum: chunk.file_checksum.clone(),
            received_chunks: HashMap::new(),
            received_bytes: 0,
            last_updated_at: Instant::now(),
        }
    }

    fn add_chunk(&mut self, chunk_index: u32, data: Vec<u8>) {
        if let Some(old) = self.received_chunks.get(&chunk_index) {
            self.received_bytes -= old.len() as u64;
        }
        self.received_bytes += data.len() as u64;
        self.received_chunks.insert(chunk_index, data);
        self.last_updated_at = Instant::now();
    }

    /// Bytes this assembly charges against the global buffer budget:
    /// payload plus a fixed per-chunk bookkeeping charge, so many tiny
    /// chunks cannot evade the budget.
    fn charged_bytes(&self) -> u64 {
        self.received_bytes
            + self.received_chunks.len() as u64 * FileTransferManager::PER_CHUNK_OVERHEAD
    }

    fn is_complete(&self) -> bool {
        if self.total_chunks == 0 || self.received_chunks.len() != self.total_chunks as usize {
            return false;
        }

        (0..self.total_chunks).all(|chunk_index| self.received_chunks.contains_key(&chunk_index))
    }

    fn progress(&self) -> FileProgress {
        FileProgress {
            file_id: String::new(), // Will be set by caller
            file_name: self.file_name.clone(),
            file_size: self.file_size,
            chunks_completed: self.received_chunks.len() as u32,
            total_chunks: self.total_chunks,
            percentage: FileProgress::calculate_percentage(
                self.received_chunks.len() as u32,
                self.total_chunks,
            ),
        }
    }

    fn reassemble(&self) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }

        // Size the buffer from bytes actually received — file_size is a
        // sender-controlled claim and must never drive an allocation.
        let mut file_data = Vec::with_capacity(self.received_bytes as usize);

        // Reassemble chunks in order
        for i in 0..self.total_chunks {
            if let Some(chunk_data) = self.received_chunks.get(&i) {
                file_data.extend_from_slice(chunk_data);
            } else {
                return None; // Missing chunk
            }
        }

        Some(file_data)
    }
}

/// File transfer manager for chunking and reassembly.
pub struct FileTransferManager {
    config: FileTransferConfig,
    active_assemblies: HashMap<String, FileAssembly>,
}

impl FileTransferManager {
    /// Floor on the average chunk payload assumed when bounding a sender's
    /// `total_chunks` claim, so per-assembly bookkeeping stays proportional
    /// to `max_file_size` even when every chunk is tiny. The SDK's own
    /// senders never chunk below `CHUNK_SIZE_BLE` (4 KiB).
    const MIN_ACCEPTED_CHUNK_PAYLOAD: u64 = 1024;

    /// Lower bound on the derived `total_chunks` cap so configurations with
    /// a small `max_file_size` still admit fine-grained chunking.
    const MIN_TOTAL_CHUNKS_CAP: u64 = 1024;

    /// Flat per-stored-chunk charge against `max_total_buffered_bytes`,
    /// approximating the bookkeeping memory the payload sum does not see
    /// (hashmap entry, allocator rounding). Without it, a flood of 1-byte
    /// chunks pins far more memory than the budget records.
    const PER_CHUNK_OVERHEAD: u64 = 64;

    /// Pseudo-sender bound to assemblies fed through the manual
    /// `process_file_chunk` FFI path, which carries no wire sender. All
    /// manual chunks share this identity for sender binding and quota. The
    /// `:` is rejected by wire `UserId` validation, so no remote peer can
    /// ever claim this identity.
    pub const MANUAL_SENDER: &'static str = "manual:ffi";

    /// Creates a new file transfer manager.
    pub fn new() -> Self {
        Self::with_config(FileTransferConfig::default())
    }

    /// Creates a new file transfer manager with custom configuration.
    ///
    /// `max_total_buffered_bytes` is clamped up to what a single
    /// `max_file_size` transfer charges (payload plus per-chunk overhead) —
    /// any lower value would make `max_file_size` unachievable.
    pub fn with_config(mut config: FileTransferConfig) -> Self {
        let min_budget = Self::min_viable_budget(&config);
        if config.max_total_buffered_bytes < min_budget {
            tracing::warn!(
                configured = config.max_total_buffered_bytes,
                clamped_to = min_budget,
                "max_total_buffered_bytes below what one max_file_size transfer charges; clamping"
            );
            config.max_total_buffered_bytes = min_budget;
        }
        Self {
            config,
            active_assemblies: HashMap::new(),
        }
    }

    /// Upper bound on a sender's `total_chunks` claim, derived from
    /// `max_file_size`. Floor division plus one — always at least the exact
    /// ceiling — without u64::div_ceil (MSRV) or overflow.
    fn derived_total_chunks_cap(max_file_size: u64) -> u64 {
        (max_file_size / Self::MIN_ACCEPTED_CHUNK_PAYLOAD)
            .saturating_add(1)
            .max(Self::MIN_TOTAL_CHUNKS_CAP)
    }

    /// The smallest budget that still admits one maximum-size transfer:
    /// its payload plus the per-chunk overhead of the most chunks such a
    /// transfer may legally claim (bounded by both the derived cap and the
    /// one-byte-per-chunk minimum).
    fn min_viable_budget(config: &FileTransferConfig) -> u64 {
        let max_chunks =
            Self::derived_total_chunks_cap(config.max_file_size).min(config.max_file_size);
        config
            .max_file_size
            .saturating_add(max_chunks.saturating_mul(Self::PER_CHUNK_OVERHEAD))
    }

    /// Chunks a file for sending.
    ///
    /// # Arguments
    ///
    /// * `file_id` - Unique identifier for this file transfer
    /// * `file_name` - Name of the file
    /// * `file_data` - Complete file data
    /// * `chunk_size_override` - If `Some`, uses this chunk size instead of the configured default.
    ///   Allows callers to select a transport-appropriate chunk size.
    ///
    /// # Returns
    ///
    /// Returns a vector of FileChunk ready to send.
    pub fn chunk_file(
        &self,
        file_id: String,
        file_name: String,
        file_data: Vec<u8>,
        chunk_size_override: Option<usize>,
    ) -> crate::Result<Vec<FileChunk>> {
        let file_size = file_data.len() as u64;

        if file_size > self.config.max_file_size {
            return Err(crate::Error::Other(format!(
                "File size {} exceeds maximum {}",
                file_size, self.config.max_file_size
            )));
        }

        if file_data.is_empty() {
            return Err(crate::Error::Other("File is empty".to_string()));
        }

        let chunk_size = chunk_size_override.unwrap_or(self.config.chunk_size);
        if chunk_size == 0 {
            return Err(crate::Error::Other(
                "Chunk size must be greater than zero".to_string(),
            ));
        }
        let file_checksum = format!("{:x}", Sha256::digest(&file_data));

        let total_chunks = ((file_size + chunk_size as u64 - 1) / chunk_size as u64) as u32;

        let mut chunks = Vec::new();

        for chunk_index in 0..total_chunks {
            let start = (chunk_index as usize) * chunk_size;
            let end = ((chunk_index as usize + 1) * chunk_size).min(file_data.len());

            let chunk = FileChunk {
                file_id: file_id.clone(),
                file_name: file_name.clone(),
                file_size,
                total_chunks,
                chunk_index,
                chunk_data: file_data[start..end].to_vec(),
                file_checksum: file_checksum.clone(),
            };

            chunks.push(chunk);
        }

        Ok(chunks)
    }

    /// Processes a received file chunk from `sender`.
    ///
    /// All wire-supplied fields (`file_size`, `total_chunks`, `chunk_data`,
    /// `file_id`) are bounded here before any assembly state is created, so
    /// a remote peer cannot force oversized allocations or unbounded
    /// bookkeeping. Assemblies are bound to the sender that started them,
    /// counted against global and per-sender caps, and the bytes buffered
    /// across all assemblies never exceed `max_total_buffered_bytes`.
    ///
    /// # Arguments
    ///
    /// * `sender` - The authenticated (or wire-claimed) sender of the chunk
    /// * `chunk` - The received file chunk
    ///
    /// # Returns
    ///
    /// Returns `Ok(FileProgress)` with current progress, or the
    /// [`ChunkRejection`] explaining why the chunk was dropped.
    pub fn process_chunk(
        &mut self,
        sender: &str,
        chunk: FileChunk,
    ) -> Result<FileProgress, ChunkRejection> {
        if chunk.total_chunks == 0 || chunk.chunk_index >= chunk.total_chunks {
            return Err(ChunkRejection::Invalid);
        }

        if chunk.file_size == 0 || chunk.file_size > self.config.max_file_size {
            tracing::warn!(
                file_id = %chunk.file_id,
                file_size = chunk.file_size,
                max_file_size = self.config.max_file_size,
                "Rejecting file chunk: claimed file_size out of bounds"
            );
            return Err(ChunkRejection::Invalid);
        }

        let max_total_chunks = Self::derived_total_chunks_cap(self.config.max_file_size);
        if chunk.total_chunks as u64 > max_total_chunks
            || chunk.total_chunks as u64 > chunk.file_size
        {
            tracing::warn!(
                file_id = %chunk.file_id,
                total_chunks = chunk.total_chunks,
                file_size = chunk.file_size,
                max_total_chunks,
                "Rejecting file chunk: claimed total_chunks out of bounds"
            );
            return Err(ChunkRejection::Invalid);
        }

        if chunk.chunk_data.is_empty() || chunk.chunk_data.len() as u64 > chunk.file_size {
            tracing::warn!(
                file_id = %chunk.file_id,
                chunk_len = chunk.chunk_data.len(),
                file_size = chunk.file_size,
                "Rejecting file chunk: chunk data length inconsistent with claimed file_size"
            );
            return Err(ChunkRejection::Invalid);
        }

        let file_id = chunk.file_id.clone();

        // A chunk for an existing assembly must be consistent with it; a
        // chunk starting a new assembly must fit within the count caps.
        // Everything is checked before any state is created or mutated.
        let replaced_charge = match self.active_assemblies.get(&file_id) {
            Some(assembly) => {
                if assembly.sender != sender {
                    tracing::warn!(
                        file_id = %file_id,
                        sender = %sender,
                        "Rejecting file chunk: sender does not match the assembly's sender"
                    );
                    return Err(ChunkRejection::SenderMismatch);
                }
                if assembly.total_chunks != chunk.total_chunks
                    || assembly.file_size != chunk.file_size
                    || assembly.file_checksum != chunk.file_checksum
                {
                    tracing::warn!(
                        file_id = %file_id,
                        "Rejecting file chunk: metadata does not match the existing assembly"
                    );
                    return Err(ChunkRejection::MetadataMismatch);
                }
                // Duplicates replace their earlier copy, so discount it
                // before checking the cumulative received bytes against the
                // claimed size.
                let replaced = assembly
                    .received_chunks
                    .get(&chunk.chunk_index)
                    .map(|c| c.len() as u64);
                if assembly.received_bytes - replaced.unwrap_or(0) + chunk.chunk_data.len() as u64
                    > assembly.file_size
                {
                    tracing::warn!(
                        file_id = %file_id,
                        received_bytes = assembly.received_bytes,
                        chunk_len = chunk.chunk_data.len(),
                        file_size = assembly.file_size,
                        "Rejecting file chunk: received bytes exceed claimed file_size"
                    );
                    return Err(ChunkRejection::Invalid);
                }
                // A replaced duplicate refunds its full charge — payload and
                // overhead — since its map entry is reused, not added.
                replaced.map_or(0, |len| len + Self::PER_CHUNK_OVERHEAD)
            }
            None => {
                if self.active_assemblies.len() >= self.config.max_concurrent_assemblies {
                    tracing::warn!(
                        file_id = %file_id,
                        active = self.active_assemblies.len(),
                        max = self.config.max_concurrent_assemblies,
                        "Rejecting file chunk: too many concurrent inbound transfers"
                    );
                    return Err(ChunkRejection::TooManyTransfers);
                }
                let sender_active = self
                    .active_assemblies
                    .values()
                    .filter(|a| a.sender == sender)
                    .count();
                if sender_active >= self.config.max_assemblies_per_sender {
                    tracing::warn!(
                        file_id = %file_id,
                        sender = %sender,
                        sender_active,
                        max = self.config.max_assemblies_per_sender,
                        "Rejecting file chunk: sender's concurrent-transfer quota reached"
                    );
                    return Err(ChunkRejection::SenderQuotaExceeded);
                }
                0
            }
        };

        // Hard memory bound: total bytes charged across every assembly,
        // where each stored chunk costs its payload plus PER_CHUNK_OVERHEAD.
        // Iterating is fine — the count caps above keep this map small.
        let buffered: u64 = self
            .active_assemblies
            .values()
            .map(FileAssembly::charged_bytes)
            .sum();
        if buffered - replaced_charge + chunk.chunk_data.len() as u64 + Self::PER_CHUNK_OVERHEAD
            > self.config.max_total_buffered_bytes
        {
            tracing::warn!(
                file_id = %file_id,
                buffered,
                chunk_len = chunk.chunk_data.len(),
                budget = self.config.max_total_buffered_bytes,
                "Rejecting file chunk: buffered-bytes budget exhausted, dropping transfer"
            );
            // The dropped chunk is never retransmitted, so this transfer can
            // no longer complete — free its partial buffer now instead of
            // letting it squat the budget until the stale sweep.
            self.active_assemblies.remove(&file_id);
            return Err(ChunkRejection::BufferBudgetExhausted);
        }

        // Get or create assembly for this file
        let assembly = self
            .active_assemblies
            .entry(file_id.clone())
            .or_insert_with(|| FileAssembly::new(sender, &chunk));

        // Add chunk
        assembly.add_chunk(chunk.chunk_index, chunk.chunk_data);

        // Get progress
        let mut progress = assembly.progress();
        progress.file_id = file_id;

        Ok(progress)
    }

    /// Checks if a file transfer is complete.
    pub fn is_complete(&self, file_id: &str) -> bool {
        self.active_assemblies
            .get(file_id)
            .map(|assembly| assembly.is_complete())
            .unwrap_or(false)
    }

    /// Finalizes a file transfer and returns the complete file data.
    ///
    /// Verifies the SHA256 checksum of the reassembled file before returning.
    ///
    /// # Arguments
    ///
    /// * `file_id` - File identifier
    ///
    /// # Returns
    ///
    /// Returns `Some(Vec<u8>)` if file is complete, its size matches the
    /// claimed `file_size`, and the checksum matches, `None` otherwise.
    pub fn finalize_file(&mut self, file_id: &str) -> Option<Vec<u8>> {
        let assembly = self.active_assemblies.get(file_id)?;
        let file_data = assembly.reassemble()?;

        // A completed transfer must deliver exactly the bytes it claimed.
        if file_data.len() as u64 != assembly.file_size {
            tracing::warn!(
                file_id = %file_id,
                claimed = assembly.file_size,
                actual = file_data.len(),
                "File size mismatch — corrupted or tampered transfer"
            );
            self.active_assemblies.remove(file_id);
            return None;
        }

        // Verify checksum before returning
        let actual_checksum = format!("{:x}", Sha256::digest(&file_data));
        if actual_checksum != assembly.file_checksum {
            tracing::warn!(
                file_id = %file_id,
                expected = %assembly.file_checksum,
                actual = %actual_checksum,
                "File checksum mismatch — corrupted or tampered transfer"
            );
            self.active_assemblies.remove(file_id);
            return None;
        }

        self.active_assemblies.remove(file_id);
        Some(file_data)
    }

    /// Removes stale/incomplete transfers that have not received any chunk
    /// updates within `max_age`, returning what was dropped so callers can
    /// surface the loss.
    pub fn cleanup_stale_transfers(&mut self, max_age: Duration) -> Vec<StaleTransfer> {
        let now = Instant::now();
        let stale_file_ids: Vec<String> = self
            .active_assemblies
            .iter()
            .filter_map(|(file_id, assembly)| {
                if now.duration_since(assembly.last_updated_at) > max_age {
                    return Some(file_id.clone());
                }
                None
            })
            .collect();

        stale_file_ids
            .into_iter()
            .filter_map(|file_id| {
                self.active_assemblies
                    .remove(&file_id)
                    .map(|assembly| StaleTransfer {
                        file_id,
                        file_name: assembly.file_name,
                        sender: assembly.sender,
                    })
            })
            .collect()
    }

    /// Gets the progress of an active file transfer.
    pub fn get_progress(&self, file_id: &str) -> Option<FileProgress> {
        self.active_assemblies.get(file_id).map(|assembly| {
            let mut progress = assembly.progress();
            progress.file_id = file_id.to_string();
            progress
        })
    }

    /// Cancels an active file transfer.
    pub fn cancel_transfer(&mut self, file_id: &str) -> bool {
        self.active_assemblies.remove(file_id).is_some()
    }

    /// Gets the number of active file transfers.
    pub fn active_transfer_count(&self) -> usize {
        self.active_assemblies.len()
    }
}

impl Default for FileTransferManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks the sliding-window state for a single outbound file transfer.
///
/// Instead of sending all chunks at once, this limits the number of in-flight
/// (unACKed) chunks to `max_in_flight`, reducing outbox pressure and providing
/// backpressure that adapts to the transport's throughput.
pub struct OutboundTransferState {
    chunks: Vec<FileChunk>,
    next_unsent: usize,
    in_flight: HashSet<u32>,
    acked: HashSet<u32>,
    max_in_flight: usize,
}

impl OutboundTransferState {
    /// Creates a new outbound transfer state with the given chunks and window size.
    pub fn new(chunks: Vec<FileChunk>, max_in_flight: usize) -> Self {
        Self {
            chunks,
            next_unsent: 0,
            in_flight: HashSet::new(),
            acked: HashSet::new(),
            max_in_flight,
        }
    }

    /// Returns the next batch of chunks that can be sent without exceeding the
    /// in-flight window. Returned chunks are marked as in-flight.
    pub fn next_chunks_to_send(&mut self) -> Vec<FileChunk> {
        let mut batch = Vec::new();
        while self.in_flight.len() < self.max_in_flight && self.next_unsent < self.chunks.len() {
            let chunk = self.chunks[self.next_unsent].clone();
            self.in_flight.insert(chunk.chunk_index);
            self.next_unsent += 1;
            batch.push(chunk);
        }
        batch
    }

    /// Records that a chunk was ACKed. Returns `true` if this was an in-flight
    /// chunk (i.e. the window has room for more).
    pub fn on_chunk_ack(&mut self, chunk_index: u32) -> bool {
        self.acked.insert(chunk_index);
        self.in_flight.remove(&chunk_index)
    }

    /// Records that a chunk failed terminally and should not be retried
    /// at the window level (the transfer will be aborted by the caller).
    pub fn on_chunk_failed(&mut self, chunk_index: u32) {
        self.in_flight.remove(&chunk_index);
    }

    /// Returns the total number of chunks in this transfer.
    pub fn total_chunks(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Returns the number of chunks that have been acknowledged.
    pub fn acked_count(&self) -> u32 {
        self.acked.len() as u32
    }

    /// Returns `true` when all chunks have been acknowledged.
    pub fn is_fully_acked(&self) -> bool {
        self.acked.len() == self.chunks.len()
    }

    /// Returns `true` when all chunks have been sent at least once
    /// (they may still be in flight awaiting ACK).
    pub fn all_sent(&self) -> bool {
        self.next_unsent >= self.chunks.len()
    }

    /// Returns `true` when the window has capacity for more chunks.
    pub fn has_capacity(&self) -> bool {
        self.in_flight.len() < self.max_in_flight && self.next_unsent < self.chunks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_small_file() {
        let manager = FileTransferManager::new();
        let file_data = b"Hello, World!".to_vec();

        let chunks = manager
            .chunk_file(
                "file1".to_string(),
                "test.txt".to_string(),
                file_data.clone(),
                None,
            )
            .unwrap();

        assert_eq!(chunks.len(), 1); // Small file = 1 chunk
        assert_eq!(chunks[0].chunk_index, 0);
        assert_eq!(chunks[0].total_chunks, 1);
        assert_eq!(chunks[0].file_size, file_data.len() as u64);
        assert_eq!(chunks[0].chunk_data, file_data);
    }

    #[test]
    fn test_chunk_large_file() {
        let config = FileTransferConfig {
            chunk_size: 100, // Small chunk for testing
            max_file_size: 1024 * 1024,
            ..FileTransferConfig::default()
        };
        let manager = FileTransferManager::with_config(config);

        let file_data = vec![0u8; 350]; // 350 bytes

        let chunks = manager
            .chunk_file("file1".to_string(), "test.bin".to_string(), file_data, None)
            .unwrap();

        assert_eq!(chunks.len(), 4); // 350 bytes / 100 = 4 chunks
        assert_eq!(chunks[0].chunk_data.len(), 100);
        assert_eq!(chunks[1].chunk_data.len(), 100);
        assert_eq!(chunks[2].chunk_data.len(), 100);
        assert_eq!(chunks[3].chunk_data.len(), 50);
    }

    #[test]
    fn test_file_too_large() {
        let config = FileTransferConfig {
            chunk_size: 1024,
            max_file_size: 1000,
            ..FileTransferConfig::default()
        };
        let manager = FileTransferManager::with_config(config);

        let file_data = vec![0u8; 2000]; // Exceeds max

        let result = manager.chunk_file(
            "file1".to_string(),
            "large.bin".to_string(),
            file_data,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_file() {
        let manager = FileTransferManager::new();
        let result = manager.chunk_file("file1".to_string(), "empty.txt".to_string(), vec![], None);
        assert!(result.is_err());
    }

    #[test]
    fn test_zero_chunk_size_override_rejected() {
        let manager = FileTransferManager::new();
        let result = manager.chunk_file(
            "file1".to_string(),
            "test.txt".to_string(),
            b"hello".to_vec(),
            Some(0),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_reassemble_file() {
        let config = FileTransferConfig {
            chunk_size: 10,
            max_file_size: 1024,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        let original_data = b"Hello, World!".to_vec();
        let chunks = manager
            .chunk_file(
                "file1".to_string(),
                "test.txt".to_string(),
                original_data.clone(),
                None,
            )
            .unwrap();

        // Process all chunks
        for chunk in chunks {
            manager.process_chunk("peer", chunk).unwrap();
        }

        assert!(manager.is_complete("file1"));

        let reassembled = manager.finalize_file("file1").unwrap();
        assert_eq!(reassembled, original_data);
    }

    #[test]
    fn test_reassemble_out_of_order() {
        let config = FileTransferConfig {
            chunk_size: 10,
            max_file_size: 1024,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        let original_data = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ".to_vec();
        let mut chunks = manager
            .chunk_file(
                "file1".to_string(),
                "alphabet.txt".to_string(),
                original_data.clone(),
                None,
            )
            .unwrap();

        // Reverse order
        chunks.reverse();

        // Process all chunks
        for chunk in chunks {
            manager.process_chunk("peer", chunk).unwrap();
        }

        assert!(manager.is_complete("file1"));

        let reassembled = manager.finalize_file("file1").unwrap();
        assert_eq!(reassembled, original_data);
    }

    #[test]
    fn test_progress_tracking() {
        let config = FileTransferConfig {
            chunk_size: 100,
            max_file_size: 1024,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        let file_data = vec![0u8; 250]; // 3 chunks
        let chunks = manager
            .chunk_file("file1".to_string(), "test.bin".to_string(), file_data, None)
            .unwrap();

        assert_eq!(chunks.len(), 3);

        // Process first chunk
        let progress = manager.process_chunk("peer", chunks[0].clone()).unwrap();
        assert_eq!(progress.chunks_completed, 1);
        assert_eq!(progress.total_chunks, 3);
        assert_eq!(progress.percentage, 33); // 1/3 ≈ 33%

        // Process second chunk
        let progress = manager.process_chunk("peer", chunks[1].clone()).unwrap();
        assert_eq!(progress.chunks_completed, 2);
        assert_eq!(progress.percentage, 66); // 2/3 ≈ 66%

        // Process third chunk
        let progress = manager.process_chunk("peer", chunks[2].clone()).unwrap();
        assert_eq!(progress.chunks_completed, 3);
        assert_eq!(progress.percentage, 100); // 3/3 = 100%

        assert!(manager.is_complete("file1"));
    }

    #[test]
    fn test_duplicate_chunk() {
        let mut manager = FileTransferManager::new();

        let file_data = b"Test data".to_vec();
        let chunks = manager
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data, None)
            .unwrap();

        // Process same chunk twice
        manager.process_chunk("peer", chunks[0].clone()).unwrap();
        manager.process_chunk("peer", chunks[0].clone()).unwrap();

        // Should still show only 1 chunk
        let progress = manager.get_progress("file1").unwrap();
        assert_eq!(progress.chunks_completed, 1);
    }

    #[test]
    fn test_multiple_files() {
        let config = FileTransferConfig {
            chunk_size: 10,
            max_file_size: 1024,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        let data1 = b"File 1 data".to_vec();
        let data2 = b"File 2 data".to_vec();

        let chunks1 = manager
            .chunk_file(
                "file1".to_string(),
                "file1.txt".to_string(),
                data1.clone(),
                None,
            )
            .unwrap();
        let chunks2 = manager
            .chunk_file(
                "file2".to_string(),
                "file2.txt".to_string(),
                data2.clone(),
                None,
            )
            .unwrap();

        // Process both files
        for chunk in chunks1 {
            manager.process_chunk("peer", chunk).unwrap();
        }
        for chunk in chunks2 {
            manager.process_chunk("peer", chunk).unwrap();
        }

        assert_eq!(manager.active_transfer_count(), 2);
        assert!(manager.is_complete("file1"));
        assert!(manager.is_complete("file2"));
    }

    #[test]
    fn test_cancel_transfer() {
        let mut manager = FileTransferManager::new();

        let file_data = b"Test data".to_vec();
        let chunks = manager
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data, None)
            .unwrap();

        manager.process_chunk("peer", chunks[0].clone()).unwrap();
        assert_eq!(manager.active_transfer_count(), 1);

        assert!(manager.cancel_transfer("file1"));
        assert_eq!(manager.active_transfer_count(), 0);
        assert!(!manager.is_complete("file1"));
    }

    #[test]
    fn test_get_progress() {
        let mut manager = FileTransferManager::new();

        let file_data = b"Test data".to_vec();
        let chunks = manager
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data, None)
            .unwrap();

        manager.process_chunk("peer", chunks[0].clone()).unwrap();

        let progress = manager.get_progress("file1").unwrap();
        assert_eq!(progress.file_id, "file1");
        assert_eq!(progress.file_name, "test.txt");
        assert!(progress.percentage > 0);
    }

    #[test]
    fn test_progress_nonexistent_file() {
        let manager = FileTransferManager::new();
        assert!(manager.get_progress("nonexistent").is_none());
    }

    #[test]
    fn test_calculate_percentage() {
        assert_eq!(FileProgress::calculate_percentage(0, 10), 0);
        assert_eq!(FileProgress::calculate_percentage(5, 10), 50);
        assert_eq!(FileProgress::calculate_percentage(10, 10), 100);
        assert_eq!(FileProgress::calculate_percentage(1, 3), 33);
    }

    #[test]
    fn test_invalid_chunk_index_rejected() {
        let mut manager = FileTransferManager::new();
        let chunks = manager
            .chunk_file(
                "file1".to_string(),
                "test.txt".to_string(),
                b"hello world".to_vec(),
                None,
            )
            .unwrap();

        let mut invalid_chunk = chunks[0].clone();
        invalid_chunk.chunk_index = invalid_chunk.total_chunks;

        assert!(manager.process_chunk("peer", invalid_chunk).is_err());
        assert!(manager.get_progress("file1").is_none());
    }

    fn attack_chunk(
        file_id: &str,
        file_size: u64,
        total_chunks: u32,
        chunk_index: u32,
        data: Vec<u8>,
    ) -> FileChunk {
        FileChunk {
            file_id: file_id.to_string(),
            file_name: "attack.bin".to_string(),
            file_size,
            total_chunks,
            chunk_index,
            chunk_data: data,
            file_checksum: "0000".to_string(),
        }
    }

    #[test]
    fn test_huge_file_size_claim_rejected() {
        // SEC-H2 regression: a single chunk claiming u64::MAX bytes used to
        // reach reassemble() and panic on Vec::with_capacity.
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("evil", u64::MAX, 1, 0, vec![0u8; 16]);
        assert!(manager.process_chunk("peer", chunk).is_err());
        assert_eq!(manager.active_transfer_count(), 0);
        assert!(!manager.is_complete("evil"));
        assert!(manager.finalize_file("evil").is_none());
    }

    #[test]
    fn test_file_size_over_configured_max_rejected() {
        let config = FileTransferConfig {
            chunk_size: 1024,
            max_file_size: 1000,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);
        let chunk = attack_chunk("f", 2000, 2, 0, vec![0u8; 100]);
        assert!(manager.process_chunk("peer", chunk).is_err());
        assert_eq!(manager.active_transfer_count(), 0);
    }

    #[test]
    fn test_zero_file_size_rejected() {
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("f", 0, 1, 0, vec![0u8; 8]);
        assert!(manager.process_chunk("peer", chunk).is_err());
    }

    #[test]
    fn test_total_chunks_exceeding_file_size_rejected() {
        // More chunks than bytes is inherently bogus (chunks are non-empty).
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("f", 10_000, 20_000, 0, vec![0u8; 8]);
        assert!(manager.process_chunk("peer", chunk).is_err());
    }

    #[test]
    fn test_total_chunks_exceeding_derived_cap_rejected() {
        // Default config: 100 MB / 1 KiB payload floor ≈ 102_400 max chunks.
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("f", 100 * 1024 * 1024, 200_000, 0, vec![0u8; 8]);
        assert!(manager.process_chunk("peer", chunk).is_err());
    }

    #[test]
    fn test_empty_chunk_data_rejected() {
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("f", 100, 2, 0, vec![]);
        assert!(manager.process_chunk("peer", chunk).is_err());
    }

    #[test]
    fn test_chunk_data_larger_than_file_size_rejected() {
        let mut manager = FileTransferManager::new();
        let chunk = attack_chunk("f", 10, 1, 0, vec![0u8; 64]);
        assert!(manager.process_chunk("peer", chunk).is_err());
    }

    #[test]
    fn test_cumulative_bytes_exceeding_claim_rejected() {
        let mut manager = FileTransferManager::new();
        // Claim 2048 bytes in 2 chunks, then send two 1500-byte chunks.
        let first = attack_chunk("f", 2048, 2, 0, vec![0u8; 1500]);
        assert!(manager.process_chunk("peer", first).is_ok());
        let second = attack_chunk("f", 2048, 2, 1, vec![0u8; 1500]);
        assert!(manager.process_chunk("peer", second).is_err());
        // The first chunk's state survives; only the offending chunk drops.
        let progress = manager.get_progress("f").unwrap();
        assert_eq!(progress.chunks_completed, 1);
    }

    #[test]
    fn test_duplicate_chunk_does_not_double_count_bytes() {
        let mut manager = FileTransferManager::new();
        // 1500 + 548 = 2048 claimed bytes; resending chunk 0 must replace
        // (not add), so the transfer still completes.
        let data0 = vec![7u8; 1500];
        let data1 = vec![9u8; 548];
        let mut whole = data0.clone();
        whole.extend_from_slice(&data1);
        let checksum = format!("{:x}", Sha256::digest(&whole));

        let make = |index: u32, data: Vec<u8>| FileChunk {
            file_id: "f".to_string(),
            file_name: "file.bin".to_string(),
            file_size: 2048,
            total_chunks: 2,
            chunk_index: index,
            chunk_data: data,
            file_checksum: checksum.clone(),
        };

        assert!(manager
            .process_chunk("peer", make(0, data0.clone()))
            .is_ok());
        assert!(manager.process_chunk("peer", make(0, data0)).is_ok());
        assert!(manager.process_chunk("peer", make(1, data1)).is_ok());
        assert!(manager.is_complete("f"));
        assert_eq!(manager.finalize_file("f").unwrap(), whole);
    }

    #[test]
    fn test_concurrent_assembly_cap_enforced() {
        let config = FileTransferConfig {
            max_concurrent_assemblies: 2,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        assert!(manager
            .process_chunk("peer", attack_chunk("f1", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
        assert!(manager
            .process_chunk("peer", attack_chunk("f2", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
        // Third simultaneous transfer is rejected...
        assert_eq!(
            manager
                .process_chunk("peer", attack_chunk("f3", 100, 2, 0, vec![0u8; 10]))
                .unwrap_err(),
            ChunkRejection::TooManyTransfers
        );
        // ...but chunks for existing assemblies still flow.
        assert!(manager
            .process_chunk("peer", attack_chunk("f1", 100, 2, 1, vec![0u8; 10]))
            .is_ok());
        // Freeing a slot re-admits new transfers.
        assert!(manager.cancel_transfer("f2"));
        assert!(manager
            .process_chunk("peer", attack_chunk("f3", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
    }

    #[test]
    fn test_sender_quota_enforced() {
        let config = FileTransferConfig {
            max_concurrent_assemblies: 4,
            max_assemblies_per_sender: 1,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        assert!(manager
            .process_chunk("alice", attack_chunk("f1", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
        // A second transfer from the same sender exceeds the quota...
        assert_eq!(
            manager
                .process_chunk("alice", attack_chunk("f2", 100, 2, 0, vec![0u8; 10]))
                .unwrap_err(),
            ChunkRejection::SenderQuotaExceeded
        );
        // ...but other senders are unaffected.
        assert!(manager
            .process_chunk("bob", attack_chunk("f3", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
    }

    #[test]
    fn test_sender_mismatch_rejected() {
        let mut manager = FileTransferManager::new();
        assert!(manager
            .process_chunk("alice", attack_chunk("f", 100, 2, 0, vec![0u8; 10]))
            .is_ok());
        // Same file_id and metadata from a different sender must not be able
        // to poison the assembly.
        assert_eq!(
            manager
                .process_chunk("mallory", attack_chunk("f", 100, 2, 1, vec![0u8; 10]))
                .unwrap_err(),
            ChunkRejection::SenderMismatch
        );
        // The original assembly is untouched.
        assert_eq!(manager.get_progress("f").unwrap().chunks_completed, 1);
    }

    #[test]
    fn test_buffer_budget_rejects_new_transfer() {
        // 1_065_536 is exactly the minimum viable budget for a 1 MB
        // max_file_size, so with_config leaves it unclamped.
        let config = FileTransferConfig {
            max_file_size: 1_000_000,
            max_total_buffered_bytes: 1_065_536,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        assert!(manager
            .process_chunk(
                "alice",
                attack_chunk("f1", 1_000_000, 2, 0, vec![0u8; 600_000])
            )
            .is_ok());
        // 600_064 + 500_064 charged would exceed the budget; the new
        // transfer is rejected before any of its state is created.
        assert_eq!(
            manager
                .process_chunk(
                    "bob",
                    attack_chunk("f2", 1_000_000, 2, 0, vec![0u8; 500_000])
                )
                .unwrap_err(),
            ChunkRejection::BufferBudgetExhausted
        );
        assert_eq!(manager.active_transfer_count(), 1);
        assert!(manager.get_progress("f2").is_none());
        // The pre-existing transfer is untouched.
        assert_eq!(manager.get_progress("f1").unwrap().chunks_completed, 1);
    }

    #[test]
    fn test_buffer_budget_drops_existing_assembly() {
        let config = FileTransferConfig {
            max_file_size: 1_000_000,
            max_total_buffered_bytes: 1_065_536,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        assert!(manager
            .process_chunk(
                "alice",
                attack_chunk("f1", 1_000_000, 3, 0, vec![0u8; 600_000])
            )
            .is_ok());
        assert!(manager
            .process_chunk("bob", attack_chunk("f2", 400_000, 2, 0, vec![0u8; 300_000]))
            .is_ok());
        // f1's next chunk busts the budget. The dropped chunk is never
        // retransmitted, so f1 can no longer complete — its partial buffer
        // is freed immediately rather than squatting the budget until the
        // stale sweep.
        assert_eq!(
            manager
                .process_chunk(
                    "alice",
                    attack_chunk("f1", 1_000_000, 3, 1, vec![0u8; 300_000])
                )
                .unwrap_err(),
            ChunkRejection::BufferBudgetExhausted
        );
        assert!(manager.get_progress("f1").is_none());
        assert_eq!(manager.active_transfer_count(), 1);
        // The freed budget re-admits other traffic.
        assert!(manager
            .process_chunk("bob", attack_chunk("f2", 400_000, 2, 1, vec![0u8; 100_000]))
            .is_ok());
    }

    #[test]
    fn test_budget_charges_per_chunk_overhead() {
        // A flood of 1-byte chunks must be stopped by its bookkeeping
        // charge, not its (negligible) payload sum. Budget 0 is clamped up
        // to the minimum viable value: 10_000 + 1024 * 64 = 75_536.
        let config = FileTransferConfig {
            max_file_size: 10_000,
            max_total_buffered_bytes: 0,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        // 1024 chunks is the derived cap for this config; f1 fills it.
        for i in 0..1024u32 {
            manager
                .process_chunk("alice", attack_chunk("f1", 10_000, 1024, i, vec![0u8; 1]))
                .unwrap();
        }

        // f2's identical flood must hit the budget long before its payload
        // (a few hundred bytes) comes anywhere near 75_536.
        let mut rejected = None;
        for i in 0..1024u32 {
            if let Err(e) =
                manager.process_chunk("bob", attack_chunk("f2", 10_000, 1024, i, vec![0u8; 1]))
            {
                rejected = Some((i, e));
                break;
            }
        }
        let (index, err) = rejected.expect("tiny-chunk flood must exhaust the budget");
        assert_eq!(err, ChunkRejection::BufferBudgetExhausted);
        let total_payload = 1024 + u64::from(index);
        assert!(
            total_payload < 2_000,
            "rejection must be overhead-driven, but {} payload bytes were buffered",
            total_payload
        );
        // The busting transfer is dropped with its chunk.
        assert!(manager.get_progress("f2").is_none());
    }

    #[test]
    fn test_manual_sender_cannot_be_a_wire_user_id() {
        // Sender binding for the manual FFI path relies on this sentinel
        // being unrepresentable as a wire UserId (':' is rejected), so no
        // remote peer can alias it.
        assert!(offline_protocol_core::UserId::new(FileTransferManager::MANUAL_SENDER).is_err());
    }

    #[test]
    fn test_file_at_exactly_max_file_size_completes() {
        // The configured budget equals max_file_size, which cannot cover
        // the per-chunk overhead — with_config clamps it up so a
        // maximum-size file still completes.
        let config = FileTransferConfig {
            chunk_size: 512,
            max_file_size: 1000,
            max_total_buffered_bytes: 1000,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);

        let file_data = vec![5u8; 1000];
        let chunks = manager
            .chunk_file(
                "f".to_string(),
                "max.bin".to_string(),
                file_data.clone(),
                None,
            )
            .unwrap();
        for chunk in chunks {
            manager.process_chunk("peer", chunk).unwrap();
        }
        assert_eq!(manager.finalize_file("f").unwrap(), file_data);
    }

    #[test]
    fn test_total_chunks_at_derived_cap_accepted() {
        // max_file_size 1 MiB → cap = 1 MiB / 1 KiB + 1 = 1025.
        let config = FileTransferConfig {
            max_file_size: 1024 * 1024,
            ..FileTransferConfig::default()
        };
        let mut manager = FileTransferManager::with_config(config);
        assert!(manager
            .process_chunk(
                "peer",
                attack_chunk("f1", 1024 * 1024, 1025, 0, vec![0u8; 8])
            )
            .is_ok());
        assert_eq!(
            manager
                .process_chunk(
                    "peer",
                    attack_chunk("f2", 1024 * 1024, 1026, 0, vec![0u8; 8])
                )
                .unwrap_err(),
            ChunkRejection::Invalid
        );
    }

    #[test]
    fn test_duplicate_chunk_replaces_data() {
        let mut manager = FileTransferManager::new();
        // A resent chunk 0 with different bytes replaces the first copy; the
        // final file must contain the replacement.
        let stale = vec![1u8; 100];
        let fresh = vec![2u8; 100];
        let tail = vec![3u8; 50];
        let mut whole = fresh.clone();
        whole.extend_from_slice(&tail);
        let checksum = format!("{:x}", Sha256::digest(&whole));

        let make = |index: u32, data: Vec<u8>| FileChunk {
            file_id: "f".to_string(),
            file_name: "file.bin".to_string(),
            file_size: 150,
            total_chunks: 2,
            chunk_index: index,
            chunk_data: data,
            file_checksum: checksum.clone(),
        };

        manager.process_chunk("peer", make(0, stale)).unwrap();
        manager.process_chunk("peer", make(0, fresh)).unwrap();
        manager.process_chunk("peer", make(1, tail)).unwrap();
        assert_eq!(manager.finalize_file("f").unwrap(), whole);
    }

    #[test]
    fn test_finalize_rejects_size_mismatch() {
        let mut manager = FileTransferManager::new();
        // Single 100-byte chunk with a valid checksum of those bytes, but a
        // claimed file_size of 2048.
        let data = vec![3u8; 100];
        let checksum = format!("{:x}", Sha256::digest(&data));
        let chunk = FileChunk {
            file_id: "f".to_string(),
            file_name: "file.bin".to_string(),
            file_size: 2048,
            total_chunks: 1,
            chunk_index: 0,
            chunk_data: data,
            file_checksum: checksum,
        };
        assert!(manager.process_chunk("peer", chunk).is_ok());
        assert!(manager.is_complete("f"));
        assert!(manager.finalize_file("f").is_none());
        // The failed transfer is discarded.
        assert_eq!(manager.active_transfer_count(), 0);
    }

    #[test]
    fn test_cleanup_stale_transfers() {
        let mut manager = FileTransferManager::new();
        let chunks = manager
            .chunk_file(
                "file1".to_string(),
                "test.txt".to_string(),
                vec![1u8; 64 * 1024],
                None,
            )
            .unwrap();

        manager.process_chunk("peer", chunks[0].clone()).unwrap();
        std::thread::sleep(Duration::from_millis(3));

        let removed = manager.cleanup_stale_transfers(Duration::from_millis(1));
        assert_eq!(
            removed,
            vec![StaleTransfer {
                file_id: "file1".to_string(),
                file_name: "test.txt".to_string(),
                sender: "peer".to_string(),
            }]
        );
        assert!(manager.get_progress("file1").is_none());
    }
}
