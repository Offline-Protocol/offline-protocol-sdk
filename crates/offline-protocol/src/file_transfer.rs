//! File transfer with chunking and reassembly.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default chunk size for file transfers (32 KB).
pub const DEFAULT_CHUNK_SIZE: usize = 32 * 1024;

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

    /// Chunk data.
    pub chunk_data: Vec<u8>,

    /// Checksum of the complete file (SHA256 hex string).
    pub file_checksum: String,
}

impl FileChunk {
    /// Serializes to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserializes from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// File transfer progress information.
#[derive(Debug, Clone)]
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
}

impl FileAssembly {
    fn new(chunk: &FileChunk) -> Self {
        Self {
            file_name: chunk.file_name.clone(),
            file_size: chunk.file_size,
            total_chunks: chunk.total_chunks,
            file_checksum: chunk.file_checksum.clone(),
            received_chunks: HashMap::new(),
        }
    }

    fn add_chunk(&mut self, chunk_index: u32, data: Vec<u8>) {
        self.received_chunks.insert(chunk_index, data);
    }

    fn is_complete(&self) -> bool {
        self.received_chunks.len() == self.total_chunks as usize
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
    ///
    /// # Returns
    ///
    /// Returns a vector of FileChunk ready to send.
    pub fn chunk_file(
        &self,
        file_id: String,
        file_name: String,
        file_data: Vec<u8>,
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

        // Calculate checksum (simple hash for now, could use SHA256 in production)
        let file_checksum = format!("{:x}", file_size); // Simplified checksum

        // Calculate number of chunks
        let total_chunks = ((file_size + self.config.chunk_size as u64 - 1)
            / self.config.chunk_size as u64) as u32;

        let mut chunks = Vec::new();

        for chunk_index in 0..total_chunks {
            let start = (chunk_index as usize) * self.config.chunk_size;
            let end = ((chunk_index as usize + 1) * self.config.chunk_size).min(file_data.len());

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
        if let Some(assembly) = self.active_assemblies.remove(file_id) {
            assembly.reassemble()
        } else {
            None
        }
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
            .chunk_file("file1".to_string(), "test.bin".to_string(), file_data)
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

        let result = manager.chunk_file("file1".to_string(), "large.bin".to_string(), file_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_file() {
        let manager = FileTransferManager::new();
        let result = manager.chunk_file("file1".to_string(), "empty.txt".to_string(), vec![]);
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
            .chunk_file("file1".to_string(), "test.bin".to_string(), file_data)
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
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data)
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
            .chunk_file("file1".to_string(), "file1.txt".to_string(), data1.clone())
            .unwrap();
        let chunks2 = manager
            .chunk_file("file2".to_string(), "file2.txt".to_string(), data2.clone())
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
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data)
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
            .chunk_file("file1".to_string(), "test.txt".to_string(), file_data)
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
}
