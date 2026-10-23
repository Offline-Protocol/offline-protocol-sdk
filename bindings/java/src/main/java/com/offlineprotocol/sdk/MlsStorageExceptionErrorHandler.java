package com.offlineprotocol.sdk;


public class MlsStorageExceptionErrorHandler implements UniffiRustCallStatusErrorHandler<MlsStorageException> {
  @Override
  public MlsStorageException lift(java.lang.foreign.MemorySegment errorBuf){
     return FfiConverterTypeMlsStorageError.INSTANCE.lift(errorBuf);
  }
}

