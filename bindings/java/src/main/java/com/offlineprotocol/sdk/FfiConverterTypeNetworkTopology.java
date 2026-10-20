package com.offlineprotocol.sdk;


public enum FfiConverterTypeNetworkTopology implements FfiConverterRustBuffer<NetworkTopology> {
  INSTANCE;

  @Override
  public NetworkTopology read(java.nio.ByteBuffer buf) {
    return new NetworkTopology(
      FfiConverterSequenceTypeNetworkNode.INSTANCE.read(buf),
      FfiConverterSequenceTypeNetworkLink.INSTANCE.read(buf),
      FfiConverterSequenceTypeMessageStats.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(NetworkTopology value) {
      return (
            FfiConverterSequenceTypeNetworkNode.INSTANCE.allocationSize(value.nodes()) +
            FfiConverterSequenceTypeNetworkLink.INSTANCE.allocationSize(value.links()) +
            FfiConverterSequenceTypeMessageStats.INSTANCE.allocationSize(value.messageStats())
      );
  }

  @Override
  public void write(NetworkTopology value, java.nio.ByteBuffer buf) {
      FfiConverterSequenceTypeNetworkNode.INSTANCE.write(value.nodes(), buf);
      FfiConverterSequenceTypeNetworkLink.INSTANCE.write(value.links(), buf);
      FfiConverterSequenceTypeMessageStats.INSTANCE.write(value.messageStats(), buf);
  }
}
