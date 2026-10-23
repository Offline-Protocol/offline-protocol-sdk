package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeBleTransportCallback extends FfiConverterCallbackInterface<BleTransportCallback> {
  static final FfiConverterTypeBleTransportCallback INSTANCE = new FfiConverterTypeBleTransportCallback();

  private FfiConverterTypeBleTransportCallback() {}
}

