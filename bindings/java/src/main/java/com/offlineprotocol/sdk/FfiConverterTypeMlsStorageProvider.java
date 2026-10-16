package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeMlsStorageProvider extends FfiConverterCallbackInterface<MlsStorageProvider> {
  static final FfiConverterTypeMlsStorageProvider INSTANCE = new FfiConverterTypeMlsStorageProvider();

  private FfiConverterTypeMlsStorageProvider() {}
}

