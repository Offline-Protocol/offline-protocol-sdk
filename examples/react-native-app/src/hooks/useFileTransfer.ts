import { useCallback, useState, useMemo } from 'react';
import {
  OfflineProtocol,
  SendFileParams,
  SendMediaParams,
  FileProgressEvent,
  FileReceivedEvent,
  MediaSentEvent,
  ContentType,
  MediaMetadata,
} from '@offline-protocol/mesh-sdk';
import { FileTransferState } from '../types/runtime';

interface UseFileTransferReturn {
  fileTransfers: FileTransferState[];
  sendFile: (params: SendFileParams) => Promise<string | null>;
  sendMedia: (params: SendMediaParams) => Promise<string | null>;
  sendImage: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  sendVoiceNote: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  sendVideo: (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata) => Promise<string | null>;
  cancelFileTransfer: (fileId: string) => Promise<boolean>;
  handleFileProgress: (event: FileProgressEvent) => void;
  handleFileReceived: (event: FileReceivedEvent) => void;
  handleMediaSent: (event: MediaSentEvent) => void;
}

export function useFileTransfer(
  protocol: OfflineProtocol | null,
  isStarted: boolean,
): UseFileTransferReturn {
  const [fileTransfers, setFileTransfers] = useState<Record<string, FileTransferState>>({});

  const handleFileProgress = useCallback((event: FileProgressEvent) => {
    setFileTransfers(prev => {
      const existing = prev[event.file_id];
      const nextState: FileTransferState = {
        fileId: event.file_id,
        fileName: existing?.fileName ?? event.file_id,
        contentType: existing?.contentType,
        direction: existing?.direction ?? 'outbound',
        percentage: event.percentage,
        chunksCompleted: event.chunks_sent,
        totalChunks: event.total_chunks,
        status: event.percentage >= 100 ? 'completed' : 'pending',
        recipient: existing?.recipient,
        sender: existing?.sender,
        lastUpdated: Date.now(),
      };
      return { ...prev, [event.file_id]: nextState };
    });
  }, []);

  const handleFileReceived = useCallback((event: FileReceivedEvent) => {
    setFileTransfers(prev => ({
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

  const handleMediaSent = useCallback((event: MediaSentEvent) => {
    setFileTransfers(prev => {
      const existing = prev[event.file_id];
      if (!existing) return prev;
      return {
        ...prev,
        [event.file_id]: {
          ...existing,
          contentType: event.content_type,
          lastUpdated: Date.now(),
        },
      };
    });
  }, []);

  const sendMedia = useCallback(
    async (params: SendMediaParams): Promise<string | null> => {
      if (!protocol || !isStarted) return null;
      try {
        const fileId = await protocol.sendMedia(params);
        setFileTransfers(prev => ({
          ...prev,
          [fileId]: {
            fileId,
            fileName: params.fileName,
            contentType: params.contentType,
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
        console.error('Failed to send media', err);
        return null;
      }
    },
    [protocol, isStarted],
  );

  const sendFile = useCallback(
    async (params: SendFileParams): Promise<string | null> => {
      return sendMedia({
        recipient: params.recipient,
        fileData: params.fileData,
        fileName: params.fileName,
        contentType: ContentType.File,
      });
    },
    [sendMedia],
  );

  const sendImage = useCallback(
    async (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata): Promise<string | null> => {
      return sendMedia({ recipient, fileData, fileName, contentType: ContentType.Image, mediaMetadata: metadata });
    },
    [sendMedia],
  );

  const sendVoiceNote = useCallback(
    async (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata): Promise<string | null> => {
      return sendMedia({ recipient, fileData, fileName, contentType: ContentType.VoiceNote, mediaMetadata: metadata });
    },
    [sendMedia],
  );

  const sendVideo = useCallback(
    async (recipient: string, fileData: string, fileName: string, metadata?: MediaMetadata): Promise<string | null> => {
      return sendMedia({ recipient, fileData, fileName, contentType: ContentType.Video, mediaMetadata: metadata });
    },
    [sendMedia],
  );

  const cancelFileTransfer = useCallback(
    async (fileId: string): Promise<boolean> => {
      if (!protocol) return false;
      try {
        const result = await protocol.cancelFileTransfer(fileId);
        if (result) {
          setFileTransfers(prev => {
            const existing = prev[fileId];
            if (!existing) return prev;
            return {
              ...prev,
              [fileId]: { ...existing, status: 'cancelled', lastUpdated: Date.now() },
            };
          });
        }
        return result;
      } catch (err) {
        console.error('Failed to cancel file transfer', err);
        return false;
      }
    },
    [protocol],
  );

  const fileTransferList = useMemo(
    () => Object.values(fileTransfers).sort((a, b) => b.lastUpdated - a.lastUpdated),
    [fileTransfers],
  );

  return {
    fileTransfers: fileTransferList,
    sendFile,
    sendMedia,
    sendImage,
    sendVoiceNote,
    sendVideo,
    cancelFileTransfer,
    handleFileProgress,
    handleFileReceived,
    handleMediaSent,
  };
}

