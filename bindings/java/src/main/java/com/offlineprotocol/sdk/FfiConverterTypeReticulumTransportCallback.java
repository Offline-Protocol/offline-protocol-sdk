package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeReticulumTransportCallback extends FfiConverterCallbackInterface<ReticulumTransportCallback> {
  static final FfiConverterTypeReticulumTransportCallback INSTANCE = new FfiConverterTypeReticulumTransportCallback();

  private FfiConverterTypeReticulumTransportCallback() {}
}

