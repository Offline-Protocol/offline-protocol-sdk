package com.offlineprotocol.sdk;


public enum FfiConverterTypeNostrMessage implements FfiConverterRustBuffer<NostrMessage> {
  INSTANCE;

  @Override
  public NostrMessage read(java.nio.ByteBuffer buf) {
    return new NostrMessage(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(NostrMessage value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.messageId()) +
            FfiConverterString.INSTANCE.allocationSize(value.eventId()) +
            FfiConverterString.INSTANCE.allocationSize(value.eventJson())
      );
  }

  @Override
  public void write(NostrMessage value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.messageId(), buf);
      FfiConverterString.INSTANCE.write(value.eventId(), buf);
      FfiConverterString.INSTANCE.write(value.eventJson(), buf);
  }
}
