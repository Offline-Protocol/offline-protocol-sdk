package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeNostrTransportCallback extends FfiConverterCallbackInterface<NostrTransportCallback> {
  static final FfiConverterTypeNostrTransportCallback INSTANCE = new FfiConverterTypeNostrTransportCallback();

  private FfiConverterTypeNostrTransportCallback() {}
}

