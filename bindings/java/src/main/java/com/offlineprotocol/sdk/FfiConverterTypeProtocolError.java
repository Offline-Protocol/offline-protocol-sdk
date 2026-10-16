package com.offlineprotocol.sdk;


public enum FfiConverterTypeProtocolError implements FfiConverterRustBuffer<ProtocolException> {
    INSTANCE;

    @Override
    public ProtocolException read(java.nio.ByteBuffer buf) {
        return switch(buf.getInt()) {
            case 1 -> new ProtocolException.NotStarted(FfiConverterString.INSTANCE.read(buf));
            case 2 -> new ProtocolException.AlreadyStarted(FfiConverterString.INSTANCE.read(buf));
            case 3 -> new ProtocolException.InvalidConfiguration(FfiConverterString.INSTANCE.read(buf));
            case 4 -> new ProtocolException.SendFailed(FfiConverterString.INSTANCE.read(buf));
            case 5 -> new ProtocolException.NoKeyPackage(FfiConverterString.INSTANCE.read(buf));
            case 6 -> new ProtocolException.SessionNotReady(FfiConverterString.INSTANCE.read(buf));
            case 7 -> new ProtocolException.EncryptFailed(FfiConverterString.INSTANCE.read(buf));
            case 8 -> new ProtocolException.InvalidState(FfiConverterString.INSTANCE.read(buf));
            case 9 -> new ProtocolException.MlsNotInitialized(FfiConverterString.INSTANCE.read(buf));
            case 10 -> new ProtocolException.MlsException(FfiConverterString.INSTANCE.read(buf));
            case 11 -> new ProtocolException.UserBlocked(FfiConverterString.INSTANCE.read(buf));
            case 12 -> new ProtocolException.MediaTransferLimit(FfiConverterString.INSTANCE.read(buf));
            case 13 -> new ProtocolException.LockPoisoned(FfiConverterString.INSTANCE.read(buf));
            case 14 -> new ProtocolException.Other(FfiConverterString.INSTANCE.read(buf));
            case 15 -> new ProtocolException.TransportException(FfiConverterString.INSTANCE.read(buf));
            case 16 -> new ProtocolException.SerializationException(FfiConverterString.INSTANCE.read(buf));
            case 17 -> new ProtocolException.ServiceException(FfiConverterString.INSTANCE.read(buf));
            case 18 -> new ProtocolException.GroupNotFound(FfiConverterString.INSTANCE.read(buf));
            case 19 -> new ProtocolException.PermissionDenied(FfiConverterString.INSTANCE.read(buf));
            case 20 -> new ProtocolException.InvalidArgument(FfiConverterString.INSTANCE.read(buf));
            case 21 -> new ProtocolException.DataDisabled(FfiConverterString.INSTANCE.read(buf));
            case 22 -> new ProtocolException.DataStorageUnavailable(FfiConverterString.INSTANCE.read(buf));
            case 23 -> new ProtocolException.DocTooLarge(FfiConverterString.INSTANCE.read(buf));
            case 24 -> new ProtocolException.DataCorrupted(FfiConverterString.INSTANCE.read(buf));
            case 25 -> new ProtocolException.TelemetryConfigInvalid(FfiConverterString.INSTANCE.read(buf));
            default -> throw new java.lang.RuntimeException("invalid error enum value, something is very wrong!!");
        };
    }

    @Override
    public long allocationSize(ProtocolException value) {
        return 4L;
    }

    @Override
    public void write(ProtocolException value, java.nio.ByteBuffer buf) {
        switch(value) {
            case ProtocolException.NotStarted x -> {
                buf.putInt(1);
            }
            case ProtocolException.AlreadyStarted x -> {
                buf.putInt(2);
            }
            case ProtocolException.InvalidConfiguration x -> {
                buf.putInt(3);
            }
            case ProtocolException.SendFailed x -> {
                buf.putInt(4);
            }
            case ProtocolException.NoKeyPackage x -> {
                buf.putInt(5);
            }
            case ProtocolException.SessionNotReady x -> {
                buf.putInt(6);
            }
            case ProtocolException.EncryptFailed x -> {
                buf.putInt(7);
            }
            case ProtocolException.InvalidState x -> {
                buf.putInt(8);
            }
            case ProtocolException.MlsNotInitialized x -> {
                buf.putInt(9);
            }
            case ProtocolException.MlsException x -> {
                buf.putInt(10);
            }
            case ProtocolException.UserBlocked x -> {
                buf.putInt(11);
            }
            case ProtocolException.MediaTransferLimit x -> {
                buf.putInt(12);
            }
            case ProtocolException.LockPoisoned x -> {
                buf.putInt(13);
            }
            case ProtocolException.Other x -> {
                buf.putInt(14);
            }
            case ProtocolException.TransportException x -> {
                buf.putInt(15);
            }
            case ProtocolException.SerializationException x -> {
                buf.putInt(16);
            }
            case ProtocolException.ServiceException x -> {
                buf.putInt(17);
            }
            case ProtocolException.GroupNotFound x -> {
                buf.putInt(18);
            }
            case ProtocolException.PermissionDenied x -> {
                buf.putInt(19);
            }
            case ProtocolException.InvalidArgument x -> {
                buf.putInt(20);
            }
            case ProtocolException.DataDisabled x -> {
                buf.putInt(21);
            }
            case ProtocolException.DataStorageUnavailable x -> {
                buf.putInt(22);
            }
            case ProtocolException.DocTooLarge x -> {
                buf.putInt(23);
            }
            case ProtocolException.DataCorrupted x -> {
                buf.putInt(24);
            }
            case ProtocolException.TelemetryConfigInvalid x -> {
                buf.putInt(25);
            }
            default -> throw new java.lang.RuntimeException("invalid error enum value, something is very wrong!!");
        };
    }
}
