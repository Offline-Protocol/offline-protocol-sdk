package com.offlineprotocol.sdk;


// The ffiConverter which transforms the Callbacks in to handles to pass to Rust.
public final class FfiConverterTypeWifiDirectTransportCallback extends FfiConverterCallbackInterface<WifiDirectTransportCallback> {
  static final FfiConverterTypeWifiDirectTransportCallback INSTANCE = new FfiConverterTypeWifiDirectTransportCallback();

  private FfiConverterTypeWifiDirectTransportCallback() {}
}
