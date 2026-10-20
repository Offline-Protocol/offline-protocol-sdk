package com.offlineprotocol.sdk;


public enum FfiConverterTypeNostrQuery implements FfiConverterRustBuffer<NostrQuery> {
  INSTANCE;

  @Override
  public NostrQuery read(java.nio.ByteBuffer buf) {
    return new NostrQuery(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(NostrQuery value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.queryId()) +
            FfiConverterString.INSTANCE.allocationSize(value.reqJson())
      );
  }

  @Override
  public void write(NostrQuery value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.queryId(), buf);
      FfiConverterString.INSTANCE.write(value.reqJson(), buf);
  }
}
