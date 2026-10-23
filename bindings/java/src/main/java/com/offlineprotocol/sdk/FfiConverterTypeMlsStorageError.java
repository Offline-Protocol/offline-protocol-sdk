package com.offlineprotocol.sdk;


public enum FfiConverterTypeMlsStorageError implements FfiConverterRustBuffer<MlsStorageException> {
    INSTANCE;

    @Override
    public MlsStorageException read(java.nio.ByteBuffer buf) {
        return switch(buf.getInt()) {
            case 1 -> new MlsStorageException.StoreFailed(FfiConverterString.INSTANCE.read(buf));
            case 2 -> new MlsStorageException.LoadFailed(FfiConverterString.INSTANCE.read(buf));
            case 3 -> new MlsStorageException.DeleteFailed(FfiConverterString.INSTANCE.read(buf));
            case 4 -> new MlsStorageException.KeyNotFound(FfiConverterString.INSTANCE.read(buf));
            case 5 -> new MlsStorageException.CorruptedData(FfiConverterString.INSTANCE.read(buf));
            default -> throw new java.lang.RuntimeException("invalid error enum value, something is very wrong!!");
        };
    }

    @Override
    public long allocationSize(MlsStorageException value) {
        return 4L;
    }

    @Override
    public void write(MlsStorageException value, java.nio.ByteBuffer buf) {
        switch(value) {
            case MlsStorageException.StoreFailed x -> {
                buf.putInt(1);
            }
            case MlsStorageException.LoadFailed x -> {
                buf.putInt(2);
            }
            case MlsStorageException.DeleteFailed x -> {
                buf.putInt(3);
            }
            case MlsStorageException.KeyNotFound x -> {
                buf.putInt(4);
            }
            case MlsStorageException.CorruptedData x -> {
                buf.putInt(5);
            }
            default -> throw new java.lang.RuntimeException("invalid error enum value, something is very wrong!!");
        };
    }
}
