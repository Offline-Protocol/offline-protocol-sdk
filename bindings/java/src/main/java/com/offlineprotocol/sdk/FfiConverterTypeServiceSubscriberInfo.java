package com.offlineprotocol.sdk;


public enum FfiConverterTypeServiceSubscriberInfo implements FfiConverterRustBuffer<ServiceSubscriberInfo> {
  INSTANCE;

  @Override
  public ServiceSubscriberInfo read(java.nio.ByteBuffer buf) {
    return new ServiceSubscriberInfo(
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterTypeSubscriptionMode.INSTANCE.read(buf),
      FfiConverterTypeSubscriptionState.INSTANCE.read(buf),
      FfiConverterString.INSTANCE.read(buf),
      FfiConverterSequenceString.INSTANCE.read(buf)
    );
  }

  @Override
  public long allocationSize(ServiceSubscriberInfo value) {
      return (
            FfiConverterString.INSTANCE.allocationSize(value.subId()) +
            FfiConverterString.INSTANCE.allocationSize(value.subscriber()) +
            FfiConverterString.INSTANCE.allocationSize(value.serviceId()) +
            FfiConverterString.INSTANCE.allocationSize(value.topic()) +
            FfiConverterTypeSubscriptionMode.INSTANCE.allocationSize(value.mode()) +
            FfiConverterTypeSubscriptionState.INSTANCE.allocationSize(value.state()) +
            FfiConverterString.INSTANCE.allocationSize(value.params()) +
            FfiConverterSequenceString.INSTANCE.allocationSize(value.keys())
      );
  }

  @Override
  public void write(ServiceSubscriberInfo value, java.nio.ByteBuffer buf) {
      FfiConverterString.INSTANCE.write(value.subId(), buf);
      FfiConverterString.INSTANCE.write(value.subscriber(), buf);
      FfiConverterString.INSTANCE.write(value.serviceId(), buf);
      FfiConverterString.INSTANCE.write(value.topic(), buf);
      FfiConverterTypeSubscriptionMode.INSTANCE.write(value.mode(), buf);
      FfiConverterTypeSubscriptionState.INSTANCE.write(value.state(), buf);
      FfiConverterString.INSTANCE.write(value.params(), buf);
      FfiConverterSequenceString.INSTANCE.write(value.keys(), buf);
  }
}
