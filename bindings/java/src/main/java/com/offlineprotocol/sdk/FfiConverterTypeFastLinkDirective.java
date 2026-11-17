package com.offlineprotocol.sdk;


public enum FfiConverterTypeFastLinkDirective implements FfiConverterRustBuffer<FastLinkDirective> {
  INSTANCE;

  @Override
  public FastLinkDirective read(java.nio.ByteBuffer buf) {
    return new FastLinkDirective(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf),
      FfiConverterOptionalShort.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterOptionalString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(FastLinkDirective value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.kind()) +
            FfiConverterString.INSTANCE.allocationSize(value.linkId()) +
            FfiConverterString.INSTANCE.allocationSize(value.peerId()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.ssid()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.passphrase()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.host()) +
            FfiConverterOptionalShort.INSTANCE.allocationSize(value.port()) +
            FfiConverterLong.INSTANCE.allocationSize(value.expiresInMs()) +
            FfiConverterOptionalString.INSTANCE.allocationSize(value.reason())
      );
  }

  @Override
  public void write(FastLinkDirective value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.kind(), buf);
      FfiConverterString.INSTANCE.write(value.linkId(), buf);
      FfiConverterString.INSTANCE.write(value.peerId(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.ssid(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.passphrase(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.host(), buf);
      FfiConverterOptionalShort.INSTANCE.write(value.port(), buf);
      FfiConverterLong.INSTANCE.write(value.expiresInMs(), buf);
      FfiConverterOptionalString.INSTANCE.write(value.reason(), buf);
  }
}
