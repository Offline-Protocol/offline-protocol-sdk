package com.offlineprotocol.sdk;


public enum FfiConverterTypeServiceSubscriptionInfo implements FfiConverterRustBuffer<ServiceSubscriptionInfo> {
  INSTANCE;

  @Override
  public ServiceSubscriptionInfo read(java.nio.ByteBuffer buf) {
    return new ServiceSubscriptionInfo(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterTypeSubscriptionMode.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ServiceSubscriptionInfo value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.subId()) +
            FfiConverterString.INSTANCE.allocationSize(value.provider()) +
            FfiConverterString.INSTANCE.allocationSize(value.serviceId()) +
            FfiConverterString.INSTANCE.allocationSize(value.topic()) +
            FfiConverterTypeSubscriptionMode.INSTANCE.allocationSize(value.mode()) +
            FfiConverterString.INSTANCE.allocationSize(value.state())
      );
  }

  @Override
  public void write(ServiceSubscriptionInfo value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.subId(), buf);
      FfiConverterString.INSTANCE.write(value.provider(), buf);
      FfiConverterString.INSTANCE.write(value.serviceId(), buf);
      FfiConverterString.INSTANCE.write(value.topic(), buf);
      FfiConverterTypeSubscriptionMode.INSTANCE.write(value.mode(), buf);
      FfiConverterString.INSTANCE.write(value.state(), buf);
  }
}
