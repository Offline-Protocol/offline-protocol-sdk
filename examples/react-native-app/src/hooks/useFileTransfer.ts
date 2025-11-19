import { useCallback, useState, useMemo } from 'react';
import {
  OfflineProtocol,
  SendFileParams,
  FileProgressEvent,
  FileReceivedEvent,
} from '@offlineprotocol/react-native';
import { FileTransferState } from '../types/runtime';

interface UseFileTransferReturn {
  fileTransfers: FileTransferState[];
  sendFile: (params: SendFileParams) => Promise<string | null>;
  cancelFileTransfer: (fileId: string) => Promise<boolean>;
  handleFileProgress: (event: FileProgressEvent) => void;
  handleFileReceived: (event: FileReceivedEvent) => void;
}

/**
 * Hook for managing file transfer operations.
 * Extracted from useOfflineProtocol to follow single responsibility principle.
 */
export function useFileTransfer(
  protocol: OfflineProtocol | null,
  isStarted: boolean
): UseFileTransferReturn {
  const [fileTransfers, setFileTransfers] = useState<Record<string, FileTransferState>>({});

  const handleFileProgress = useCallback((event: FileProgressEvent) => {
    setFileTransfers((prev) => {
      const existing = prev[event.file_id];
      const nextState: FileTransferState = {
        fileId: event.file_id,
        fileName: existing?.fileName ?? event.file_id,
        direction: existing?.direction ?? 'outbound',
        percentage: event.percentage,
        chunksCompleted: event.chunks_sent,
        totalChunks: event.total_chunks,
        status: event.percentage >= 100 ? 'completed' : 'pending',
        recipient: existing?.recipient,
        sender: existing?.sender,
        lastUpdated: Date.now(),
      };
      return {
        ...prev,
        [event.file_id]: nextState,
      };
    });
  }, []);

  const handleFileReceived = useCallback((event: FileReceivedEvent) => {
    setFileTransfers((prev) => ({
      ...prev,
      [event.file_id]: {
        fileId: event.file_id,
        fileName: event.file_name,
        direction: 'inbound',
        percentage: 100,
        chunksCompleted: prev[event.file_id]?.chunksCompleted ?? 0,
        totalChunks: prev[event.file_id]?.totalChunks ?? 0,
        status: 'completed',
        sender: event.sender,
        lastUpdated: Date.now(),
      },
    }));
  }, []);

  const sendFile = useCallback(
    async (params: SendFileParams): Promise<string | null> => {
      if (!protocol) {
        return null;
      }
      if (!isStarted) {
        return null;
      }
      try {
        const fileId = await protocol.sendFile(params);
        const fileName =
          params.fileName ?? params.filePath.split(/[\\/]/).pop() ?? params.filePath;
        setFileTransfers((prev) => ({
          ...prev,
          [fileId]: {
            fileId,
            fileName,
            direction: 'outbound',
            percentage: 0,
            chunksCompleted: 0,
            totalChunks: 0,
            status: 'pending',
            recipient: params.recipient,
            lastUpdated: Date.now(),
          },
        }));
        return fileId;
      } catch (err) {
        console.error('Failed to send file', err);
        return null;
      }
    },
    [protocol, isStarted]
  );

  const cancelFileTransfer = useCallback(
    async (fileId: string): Promise<boolean> => {
      if (!protocol) {
        return false;
      }
      try {
        const result = await protocol.cancelFileTransfer(fileId);
        if (result) {
          setFileTransfers((prev) => {
            const existing = prev[fileId];
            if (!existing) {
              return prev;
            }
            return {
              ...prev,
              [fileId]: {
                ...existing,
                status: 'cancelled',
                lastUpdated: Date.now(),
              },
            };
          });
        }
        return result;
      } catch (err) {
        console.error('Failed to cancel file transfer', err);
        return false;
      }
    },
    [protocol]
  );

  const fileTransferList = useMemo(() => {
    return Object.values(fileTransfers).sort((a, b) => b.lastUpdated - a.lastUpdated);
  }, [fileTransfers]);

  return {
    fileTransfers: fileTransferList,
    sendFile,
    cancelFileTransfer,
    handleFileProgress,
    handleFileReceived,
  };
}

