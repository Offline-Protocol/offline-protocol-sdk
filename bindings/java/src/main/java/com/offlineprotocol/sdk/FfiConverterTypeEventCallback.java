package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeEventCallback extends FfiConverterCallbackInterface<EventCallback> {
  static final FfiConverterTypeEventCallback INSTANCE = new FfiConverterTypeEventCallback();

  private FfiConverterTypeEventCallback() {}
}

