package com.offlineprotocol.sdk;


public enum FfiConverterTypeServicePublishReport implements FfiConverterRustBuffer<ServicePublishReport> {
  INSTANCE;

  @Override
  public ServicePublishReport read(java.nio.ByteBuffer buf) {
    return new ServicePublishReport(
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf),
      FfiConverterInteger.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ServicePublishReport value) {
      return (
            FfiConverterInteger.INSTANCE.allocationSize(value.subscribers()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.sentLive()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.queued()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.coalesced()) +
            FfiConverterInteger.INSTANCE.allocationSize(value.failed())
      );
  }

  @Override
  public void write(ServicePublishReport value, java.nio.ByteBuffer buf) {
      FfiConverterInteger.INSTANCE.write(value.subscribers(), buf);
      FfiConverterInteger.INSTANCE.write(value.sentLive(), buf);
      FfiConverterInteger.INSTANCE.write(value.queued(), buf);
      FfiConverterInteger.INSTANCE.write(value.coalesced(), buf);
      FfiConverterInteger.INSTANCE.write(value.failed(), buf);
  }
}
