package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeProtocolStateStorageProvider extends FfiConverterCallbackInterface<ProtocolStateStorageProvider> {
  static final FfiConverterTypeProtocolStateStorageProvider INSTANCE = new FfiConverterTypeProtocolStateStorageProvider();

  private FfiConverterTypeProtocolStateStorageProvider() {}
}

