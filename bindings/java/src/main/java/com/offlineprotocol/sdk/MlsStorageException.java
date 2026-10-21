package com.offlineprotocol.sdk;



public class MlsStorageException extends java.lang.Exception {
    private MlsStorageException(java.lang.String message) {
      super(message);
    }

    public static class StoreFailed extends MlsStorageException {
      public StoreFailed(java.lang.String message) {
        super(message);
      }
    }
    public static class LoadFailed extends MlsStorageException {
      public LoadFailed(java.lang.String message) {
        super(message);
      }
    }
    public static class DeleteFailed extends MlsStorageException {
      public DeleteFailed(java.lang.String message) {
        super(message);
      }
    }
    public static class KeyNotFound extends MlsStorageException {
      public KeyNotFound(java.lang.String message) {
        super(message);
      }
    }
    public static class CorruptedData extends MlsStorageException {
      public CorruptedData(java.lang.String message) {
        super(message);
      }
    }
    
}

