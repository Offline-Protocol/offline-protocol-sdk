package com.offlineprotocol.sdk;


public enum FfiConverterSequenceTypeNetworkNode implements FfiConverterRustBuffer<java.util.List<NetworkNode>> {
  INSTANCE;

  @Override
  public java.util.List<NetworkNode> read(java.nio.ByteBuffer buf) {
    int len = buf.getInt();
    return java.util.stream.IntStream.range(0, len).mapToObj(_i -> FfiConverterTypeNetworkNode.INSTANCE.read(buf)).toList();
  }

  @Override
  public long allocationSize(java.util.List<NetworkNode> value) {
    long sizeForLength = 4L;
    long sizeForItems = value.stream().mapToLong(inner -> FfiConverterTypeNetworkNode.INSTANCE.allocationSize(inner)).sum();
    return sizeForLength + sizeForItems;
  }

  @Override
  public void write(java.util.List<NetworkNode> value, java.nio.ByteBuffer buf) {
    buf.putInt(value.size());
    value.forEach(inner -> FfiConverterTypeNetworkNode.INSTANCE.write(inner, buf));
  }
}
