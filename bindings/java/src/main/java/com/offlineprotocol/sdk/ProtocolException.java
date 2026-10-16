package com.offlineprotocol.sdk;



public class ProtocolException extends java.lang.Exception {
    private ProtocolException(java.lang.String message) {
      super(message);
    }

    public static class NotStarted extends ProtocolException {
      public NotStarted(java.lang.String message) {
        super(message);
      }
    }
    public static class AlreadyStarted extends ProtocolException {
      public AlreadyStarted(java.lang.String message) {
        super(message);
      }
    }
    public static class InvalidConfiguration extends ProtocolException {
      public InvalidConfiguration(java.lang.String message) {
        super(message);
      }
    }
    public static class SendFailed extends ProtocolException {
      public SendFailed(java.lang.String message) {
        super(message);
      }
    }
    public static class NoKeyPackage extends ProtocolException {
      public NoKeyPackage(java.lang.String message) {
        super(message);
      }
    }
    public static class SessionNotReady extends ProtocolException {
      public SessionNotReady(java.lang.String message) {
        super(message);
      }
    }
    public static class EncryptFailed extends ProtocolException {
      public EncryptFailed(java.lang.String message) {
        super(message);
      }
    }
    public static class InvalidState extends ProtocolException {
      public InvalidState(java.lang.String message) {
        super(message);
      }
    }
    public static class MlsNotInitialized extends ProtocolException {
      public MlsNotInitialized(java.lang.String message) {
        super(message);
      }
    }
    public static class MlsException extends ProtocolException {
      public MlsException(java.lang.String message) {
        super(message);
      }
    }
    public static class UserBlocked extends ProtocolException {
      public UserBlocked(java.lang.String message) {
        super(message);
      }
    }
    public static class MediaTransferLimit extends ProtocolException {
      public MediaTransferLimit(java.lang.String message) {
        super(message);
      }
    }
    public static class LockPoisoned extends ProtocolException {
      public LockPoisoned(java.lang.String message) {
        super(message);
      }
    }
    public static class Other extends ProtocolException {
      public Other(java.lang.String message) {
        super(message);
      }
    }
    public static class TransportException extends ProtocolException {
      public TransportException(java.lang.String message) {
        super(message);
      }
    }
    public static class SerializationException extends ProtocolException {
      public SerializationException(java.lang.String message) {
        super(message);
      }
    }
    public static class ServiceException extends ProtocolException {
      public ServiceException(java.lang.String message) {
        super(message);
      }
    }
    public static class GroupNotFound extends ProtocolException {
      public GroupNotFound(java.lang.String message) {
        super(message);
      }
    }
    public static class PermissionDenied extends ProtocolException {
      public PermissionDenied(java.lang.String message) {
        super(message);
      }
    }
    public static class InvalidArgument extends ProtocolException {
      public InvalidArgument(java.lang.String message) {
        super(message);
      }
    }
    public static class DataDisabled extends ProtocolException {
      public DataDisabled(java.lang.String message) {
        super(message);
      }
    }
    public static class DataStorageUnavailable extends ProtocolException {
      public DataStorageUnavailable(java.lang.String message) {
        super(message);
      }
    }
    public static class DocTooLarge extends ProtocolException {
      public DocTooLarge(java.lang.String message) {
        super(message);
      }
    }
    public static class DataCorrupted extends ProtocolException {
      public DataCorrupted(java.lang.String message) {
        super(message);
      }
    }
    public static class TelemetryConfigInvalid extends ProtocolException {
      public TelemetryConfigInvalid(java.lang.String message) {
        super(message);
      }
    }
    
}

