//! File transfer with fragmentation and reassembly

use offline_protocol_core::{FileChunk, FileMessage, MessageId};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

/// Progress callback for file transfers
pub type ProgressCallback = Arc<dyn Fn(FileProgress) + Send + Sync>;

/// File transfer progress information
#[derive(Debug, Clone)]
pub struct FileProgress {
    pub percentage: f64,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub current_chunk: u32,
    pub total_chunks: u32,
}

/// File being transferred
pub struct TransferringFile {
    pub file_id: MessageId,
    pub metadata: FileMessage,
    pub chunks: Vec<FileChunk>,
    pub progress_callback: Option<ProgressCallback>,
}

/// File being received
struct ReceivingFile {
    metadata: FileMessage,
    received_chunks: HashMap<u32, FileChunk>,
}

/// Manages file fragmentation and reassembly
pub struct FileTransferManager {
    chunk_size: usize,
    sending_files: Arc<RwLock<HashMap<MessageId, TransferringFile>>>,
    receiving_files: Arc<RwLock<HashMap<MessageId, ReceivingFile>>>,
}

impl FileTransferManager {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            chunk_size,
            sending_files: Arc::new(RwLock::new(HashMap::new())),
            receiving_files: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Fragment a file into chunks
    pub fn fragment_file(
        &self,
        file_id: MessageId,
        name: String,
        data: Vec<u8>,
        mime_type: String,
        progress_callback: Option<ProgressCallback>,
    ) -> TransferringFile {
        let total_size = data.len() as u64;
        let total_chunks = ((data.len() + self.chunk_size - 1) / self.chunk_size) as u32;

        debug!(
            "Fragmenting file {} into {} chunks ({} bytes each)",
            name, total_chunks, self.chunk_size
        );

        let metadata = FileMessage {
            name,
            size: total_size,
            mime_type,
            total_chunks,
            metadata: HashMap::new(),
        };

        let mut chunks = Vec::new();
        for (i, chunk_data) in data.chunks(self.chunk_size).enumerate() {
            let chunk = FileChunk {
                file_id,
                chunk_index: i as u32,
                total_chunks,
                data: chunk_data.to_vec(),
                checksum: Self::calculate_checksum(chunk_data),
            };
            chunks.push(chunk);
        }

        TransferringFile {
            file_id,
            metadata,
            chunks,
            progress_callback,
        }
    }

    /// Register a file transfer
    pub fn register_sending(&self, file: TransferringFile) {
        self.sending_files.write().insert(file.file_id, file);
    }

    /// Get the next chunk to send for a file
    pub fn get_next_chunk(&self, file_id: MessageId, chunk_index: u32) -> Option<FileChunk> {
        let files = self.sending_files.read();
        files.get(&file_id).and_then(|f| {
            f.chunks.get(chunk_index as usize).cloned()
        })
    }

    /// Update progress for a sent chunk
    pub fn update_send_progress(&self, file_id: MessageId, chunk_index: u32) {
        let files = self.sending_files.read();
        if let Some(file) = files.get(&file_id) {
            let progress = FileProgress {
                percentage: ((chunk_index + 1) as f64 / file.chunks.len() as f64) * 100.0,
                bytes_sent: (chunk_index + 1) as u64 * self.chunk_size as u64,
                total_bytes: file.metadata.size,
                current_chunk: chunk_index + 1,
                total_chunks: file.metadata.total_chunks,
            };

            if let Some(callback) = &file.progress_callback {
                callback(progress);
            }
        }
    }

    /// Handle received file metadata
    pub fn handle_file_metadata(&self, file_id: MessageId, metadata: FileMessage) {
        debug!("Receiving file: {} ({} bytes, {} chunks)",
            metadata.name, metadata.size, metadata.total_chunks);

        let receiving = ReceivingFile {
            metadata,
            received_chunks: HashMap::new(),
        };

        self.receiving_files.write().insert(file_id, receiving);
    }

    /// Handle received file chunk
    pub fn handle_file_chunk(&self, chunk: FileChunk) -> Option<Vec<u8>> {
        let mut files = self.receiving_files.write();
        
        if let Some(receiving) = files.get_mut(&chunk.file_id) {
            // Verify checksum
            let calculated = Self::calculate_checksum(&chunk.data);
            if calculated != chunk.checksum {
                warn!("Checksum mismatch for chunk {}", chunk.chunk_index);
                return None;
            }

            let file_id = chunk.file_id;
            let chunk_count = receiving.received_chunks.len() + 1;
            let total_chunks = receiving.metadata.total_chunks;

            // Store chunk
            receiving.received_chunks.insert(chunk.chunk_index, chunk);

            debug!(
                "Received chunk {}/{} for file {}",
                chunk_count,
                total_chunks,
                file_id
            );

            // Check if all chunks received
            if receiving.received_chunks.len() == total_chunks as usize {
                debug!("All chunks received, reassembling file");
                return Some(self.reassemble_file(&receiving));
            }
        }

        None
    }

    /// Reassemble a file from chunks
    fn reassemble_file(&self, receiving: &ReceivingFile) -> Vec<u8> {
        let mut data = Vec::with_capacity(receiving.metadata.size as usize);
        
        for i in 0..receiving.metadata.total_chunks {
            if let Some(chunk) = receiving.received_chunks.get(&i) {
                data.extend_from_slice(&chunk.data);
            }
        }

        data
    }

    /// Calculate checksum for chunk data (simple CRC32)
    fn calculate_checksum(data: &[u8]) -> u32 {
        // Simple checksum (in production, use CRC32 or similar)
        data.iter().fold(0u32, |acc, &byte| {
            acc.wrapping_add(byte as u32)
        })
    }

    /// Remove completed transfer
    pub fn remove_sending(&self, file_id: MessageId) {
        self.sending_files.write().remove(&file_id);
    }

    /// Remove completed receiving
    pub fn remove_receiving(&self, file_id: MessageId) {
        self.receiving_files.write().remove(&file_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_fragmentation() {
        let manager = FileTransferManager::new(512);
        let file_id = MessageId::new();
        let data = vec![0u8; 1500]; // 3 chunks

        let transfer = manager.fragment_file(
            file_id,
            "test.bin".to_string(),
            data.clone(),
            "application/octet-stream".to_string(),
            None,
        );

        assert_eq!(transfer.chunks.len(), 3);
        assert_eq!(transfer.metadata.total_chunks, 3);
        assert_eq!(transfer.metadata.size, 1500);
    }

    #[test]
    fn test_file_reassembly() {
        let manager = FileTransferManager::new(512);
        let file_id = MessageId::new();
        let original_data = b"Hello, World! This is a test file.".to_vec();

        // Fragment
        let transfer = manager.fragment_file(
            file_id,
            "test.txt".to_string(),
            original_data.clone(),
            "text/plain".to_string(),
            None,
        );

        // Register metadata
        manager.handle_file_metadata(file_id, transfer.metadata.clone());

        // Send chunks
        let mut reassembled = None;
        for chunk in transfer.chunks {
            reassembled = manager.handle_file_chunk(chunk);
        }

        assert!(reassembled.is_some());
        assert_eq!(reassembled.unwrap(), original_data);
    }
}

