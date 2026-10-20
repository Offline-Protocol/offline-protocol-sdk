package com.offlineprotocol.sdk;


public class ProtocolExceptionErrorHandler implements UniffiRustCallStatusErrorHandler<ProtocolException> {
  @Override
  public ProtocolException lift(java.lang.foreign.MemorySegment errorBuf){
     return FfiConverterTypeProtocolError.INSTANCE.lift(errorBuf);
  }
}

