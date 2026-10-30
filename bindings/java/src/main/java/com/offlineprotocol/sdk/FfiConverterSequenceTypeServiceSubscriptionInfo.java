package com.offlineprotocol.sdk;


public enum FfiConverterSequenceTypeServiceSubscriptionInfo implements FfiConverterRustBuffer<java.util.List<ServiceSubscriptionInfo>> {
  INSTANCE;

  @Override
  public java.util.List<ServiceSubscriptionInfo> read(java.nio.ByteBuffer buf) {
    int len = buf.getInt();
    return java.util.stream.IntStream.range(0, len).mapToObj(_i -> FfiConverterTypeServiceSubscriptionInfo.INSTANCE.read(buf)).toList();
  }

  @Override
  public long allocationSize(java.util.List<ServiceSubscriptionInfo> value) {
    long sizeForLength = 4L;
    long sizeForItems = value.stream().mapToLong(inner -> FfiConverterTypeServiceSubscriptionInfo.INSTANCE.allocationSize(inner)).sum();
    return sizeForLength + sizeForItems;
  }

  @Override
  public void write(java.util.List<ServiceSubscriptionInfo> value, java.nio.ByteBuffer buf) {
    buf.putInt(value.size());
    value.forEach(inner -> FfiConverterTypeServiceSubscriptionInfo.INSTANCE.write(inner, buf));
  }
}
