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
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_file_size: 100 * 1024 * 1024, // 100 MB
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
        let file_id = String::from_utf8(read_bytes(&mut pos, file_id_len)?)
            .map_err(|e| format!("invalid file_id UTF-8: {}", e))?;

        let file_name_len = read_u32(&mut pos)? as usize;
        let file_name = String::from_utf8(read_bytes(&mut pos, file_name_len)?)
            .map_err(|e| format!("invalid file_name UTF-8: {}", e))?;

        let file_size = read_u64(&mut pos)?;
        let total_chunks = read_u32(&mut pos)?;
        let chunk_index = read_u32(&mut pos)?;

        let chunk_data_len = read_u32(&mut pos)? as usize;
        let chunk_data = read_bytes(&mut pos, chunk_data_len)?;

        let checksum_len = read_u32(&mut pos)? as usize;
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

/// Tracks file reassembly state.
struct FileAssembly {
    file_name: String,
    file_size: u64,
    total_chunks: u32,
    file_checksum: String,
    received_chunks: HashMap<u32, Vec<u8>>,
    last_updated_at: Instant,
}

impl FileAssembly {
    fn new(chunk: &FileChunk) -> Self {
        Self {
            file_name: chunk.file_name.clone(),
            file_size: chunk.file_size,
            total_chunks: chunk.total_chunks,
            file_checksum: chunk.file_checksum.clone(),
            received_chunks: HashMap::new(),
            last_updated_at: Instant::now(),
        }
    }

    fn add_chunk(&mut self, chunk_index: u32, data: Vec<u8>) {
        self.received_chunks.insert(chunk_index, data);
        self.last_updated_at = Instant::now();
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

        let mut file_data = Vec::with_capacity(self.file_size as usize);

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
    /// Creates a new file transfer manager.
    pub fn new() -> Self {
        Self::with_config(FileTransferConfig::default())
    }

    /// Creates a new file transfer manager with custom configuration.
    pub fn with_config(config: FileTransferConfig) -> Self {
        Self {
            config,
            active_assemblies: HashMap::new(),
        }
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

    /// Processes a received file chunk.
    ///
    /// # Arguments
    ///
    /// * `chunk` - The received file chunk
    ///
    /// # Returns
    ///
    /// Returns `Some(FileProgress)` with current progress, or `None` if chunk is invalid.
    pub fn process_chunk(&mut self, chunk: FileChunk) -> Option<FileProgress> {
        if chunk.total_chunks == 0 || chunk.chunk_index >= chunk.total_chunks {
            return None;
        }

        let file_id = chunk.file_id.clone();

        // Get or create assembly for this file
        let assembly = self
            .active_assemblies
            .entry(file_id.clone())
            .or_insert_with(|| FileAssembly::new(&chunk));

        // Validate chunk belongs to same file
        if assembly.total_chunks != chunk.total_chunks
            || assembly.file_size != chunk.file_size
            || assembly.file_checksum != chunk.file_checksum
        {
            return None; // Mismatched file metadata
        }

        // Add chunk
        assembly.add_chunk(chunk.chunk_index, chunk.chunk_data);

        // Get progress
        let mut progress = assembly.progress();
        progress.file_id = file_id;

        Some(progress)
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
    /// # Arguments
    ///
    /// * `file_id` - File identifier
    ///
    /// # Returns
    ///
    /// Returns `Some(Vec<u8>)` if file is complete, `None` otherwise.
    pub fn finalize_file(&mut self, file_id: &str) -> Option<Vec<u8>> {
        let file_data = self
            .active_assemblies
            .get(file_id)
            .and_then(FileAssembly::reassemble)?;

        self.active_assemblies.remove(file_id);
        Some(file_data)
    }

    /// Removes stale/incomplete transfers that have not received any chunk
    /// updates within `max_age`.
    pub fn cleanup_stale_transfers(&mut self, max_age: Duration) -> Vec<String> {
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

        for file_id in &stale_file_ids {
            self.active_assemblies.remove(file_id);
        }

        stale_file_ids
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
            manager.process_chunk(chunk);
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
            manager.process_chunk(chunk);
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
        };
        let mut manager = FileTransferManager::with_config(config);

        let file_data = vec![0u8; 250]; // 3 chunks
        let chunks = manager
            .chunk_file("file1".to_string(), "test.bin".to_string(), file_data, None)
            .unwrap();

        assert_eq!(chunks.len(), 3);

        // Process first chunk
        let progress = manager.process_chunk(chunks[0].clone()).unwrap();
        assert_eq!(progress.chunks_completed, 1);
        assert_eq!(progress.total_chunks, 3);
        assert_eq!(progress.percentage, 33); // 1/3 ≈ 33%

        // Process second chunk
        let progress = manager.process_chunk(chunks[1].clone()).unwrap();
        assert_eq!(progress.chunks_completed, 2);
        assert_eq!(progress.percentage, 66); // 2/3 ≈ 66%

        // Process third chunk
        let progress = manager.process_chunk(chunks[2].clone()).unwrap();
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
        manager.process_chunk(chunks[0].clone());
        manager.process_chunk(chunks[0].clone());

        // Should still show only 1 chunk
        let progress = manager.get_progress("file1").unwrap();
        assert_eq!(progress.chunks_completed, 1);
    }

    #[test]
    fn test_multiple_files() {
        let config = FileTransferConfig {
            chunk_size: 10,
            max_file_size: 1024,
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
            manager.process_chunk(chunk);
        }
        for chunk in chunks2 {
            manager.process_chunk(chunk);
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

        manager.process_chunk(chunks[0].clone());
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

        manager.process_chunk(chunks[0].clone());

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

        assert!(manager.process_chunk(invalid_chunk).is_none());
        assert!(manager.get_progress("file1").is_none());
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

        manager.process_chunk(chunks[0].clone());
        std::thread::sleep(Duration::from_millis(3));

        let removed = manager.cleanup_stale_transfers(Duration::from_millis(1));
        assert_eq!(removed, vec!["file1".to_string()]);
        assert!(manager.get_progress("file1").is_none());
    }
}
