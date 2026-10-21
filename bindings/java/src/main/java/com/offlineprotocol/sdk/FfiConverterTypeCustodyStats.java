package com.offlineprotocol.sdk;


public enum FfiConverterTypeCustodyStats implements FfiConverterRustBuffer<CustodyStats> {
  INSTANCE;

  @Override
  public CustodyStats read(java.nio.ByteBuffer buf) {
    return new CustodyStats(
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf),
      FfiConverterLong.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(CustodyStats value) {
      return (
            FfiConverterLong.INSTANCE.allocationSize(value.held()) +
            FfiConverterLong.INSTANCE.allocationSize(value.heldBytes()) +
            FfiConverterLong.INSTANCE.allocationSize(value.accepted()) +
            FfiConverterLong.INSTANCE.allocationSize(value.delivered()) +
            FfiConverterLong.INSTANCE.allocationSize(value.reOriginated()) +
            FfiConverterLong.INSTANCE.allocationSize(value.expired()) +
            FfiConverterLong.INSTANCE.allocationSize(value.duplicates()) +
            FfiConverterLong.INSTANCE.allocationSize(value.evicted()) +
            FfiConverterLong.INSTANCE.allocationSize(value.receiptsSent()) +
            FfiConverterLong.INSTANCE.allocationSize(value.receiptsDropped()) +
            FfiConverterLong.INSTANCE.allocationSize(value.receiptsReceived()) +
            FfiConverterLong.INSTANCE.allocationSize(value.receiptsIgnored()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedDisabled()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedNoRequest()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedUnknownClass()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedNotSealed()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedUnprovenPeer()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedNotDepositor()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedStranger()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedDepositorFull()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedStoreFull()) +
            FfiConverterLong.INSTANCE.allocationSize(value.refusedBattery())
      );
  }

  @Override
  public void write(CustodyStats value, java.nio.ByteBuffer buf) {
      FfiConverterLong.INSTANCE.write(value.held(), buf);
      FfiConverterLong.INSTANCE.write(value.heldBytes(), buf);
      FfiConverterLong.INSTANCE.write(value.accepted(), buf);
      FfiConverterLong.INSTANCE.write(value.delivered(), buf);
      FfiConverterLong.INSTANCE.write(value.reOriginated(), buf);
      FfiConverterLong.INSTANCE.write(value.expired(), buf);
      FfiConverterLong.INSTANCE.write(value.duplicates(), buf);
      FfiConverterLong.INSTANCE.write(value.evicted(), buf);
      FfiConverterLong.INSTANCE.write(value.receiptsSent(), buf);
      FfiConverterLong.INSTANCE.write(value.receiptsDropped(), buf);
      FfiConverterLong.INSTANCE.write(value.receiptsReceived(), buf);
      FfiConverterLong.INSTANCE.write(value.receiptsIgnored(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedDisabled(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedNoRequest(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedUnknownClass(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedNotSealed(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedUnprovenPeer(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedNotDepositor(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedStranger(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedDepositorFull(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedStoreFull(), buf);
      FfiConverterLong.INSTANCE.write(value.refusedBattery(), buf);
  }
}
