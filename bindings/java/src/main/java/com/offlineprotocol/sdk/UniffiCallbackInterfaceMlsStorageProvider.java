package com.offlineprotocol.sdk;


// Put the implementation in an object so we don't pollute the top-level namespace
public class UniffiCallbackInterfaceMlsStorageProvider {
    public static final UniffiCallbackInterfaceMlsStorageProvider INSTANCE = new UniffiCallbackInterfaceMlsStorageProvider();
    java.lang.foreign.MemorySegment vtable;

    UniffiCallbackInterfaceMlsStorageProvider() {
        // Use Arena.global() for vtable and upcall stubs - they live for the program lifetime.
        // Arena.ofAuto() stubs can be GC'd since storing an address in a struct doesn't
        // prevent the Arena from being collected.
        vtable = java.lang.foreign.Arena.global().allocate(UniffiVTableCallbackInterfaceMlsStorageProvider.LAYOUT);
        UniffiVTableCallbackInterfaceMlsStorageProvider.setuniffiFree(vtable, UniffiCallbackInterfaceFree.toUpcallStub(UniffiFree.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceMlsStorageProvider.setuniffiClone(vtable, UniffiCallbackInterfaceClone.toUpcallStub(UniffiClone.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceMlsStorageProvider.setstore(vtable, UniffiCallbackInterfaceMlsStorageProviderMethod0.toUpcallStub(StoreCallback.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceMlsStorageProvider.setload(vtable, UniffiCallbackInterfaceMlsStorageProviderMethod1.toUpcallStub(LoadCallback.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceMlsStorageProvider.setdelete(vtable, UniffiCallbackInterfaceMlsStorageProviderMethod2.toUpcallStub(DeleteCallback.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceMlsStorageProvider.setlistKeys(vtable, UniffiCallbackInterfaceMlsStorageProviderMethod3.toUpcallStub(ListKeysCallback.INSTANCE, java.lang.foreign.Arena.global()));
    }

    // Registers the foreign callback with the Rust side.
    void register() {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_init_callback_vtable_mlsstorageprovider(vtable);
    }
    
    public static final class StoreCallback implements UniffiCallbackInterfaceMlsStorageProviderMethod0.Fn {
        public static final StoreCallback INSTANCE = new StoreCallback();
        private StoreCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment keyType,java.lang.foreign.MemorySegment keyId,java.lang.foreign.MemorySegment data,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.get(uniffiHandle);
            java.util.concurrent.Callable<java.lang.Void> _uniffiMakeCall = () -> {
                _uniffiObj.store(
                    FfiConverterString.INSTANCE.lift(keyType),
                    FfiConverterString.INSTANCE.lift(keyId),
                    FfiConverterSequenceByte.INSTANCE.lift(data)
                );
                return null;
            };
            java.util.function.Consumer<java.lang.Void> _uniffiWriteReturn = (_uniffiNothing) -> {};
            UniffiHelpers.uniffiTraitInterfaceCallWithError(
                _uniffiCallStatus,
                _uniffiMakeCall,
                _uniffiWriteReturn,
                (MlsStorageException _uniffiErr) -> { return FfiConverterTypeMlsStorageError.INSTANCE.lower(_uniffiErr); },
                MlsStorageException.class
            );
        }
    }
    
    public static final class LoadCallback implements UniffiCallbackInterfaceMlsStorageProviderMethod1.Fn {
        public static final LoadCallback INSTANCE = new LoadCallback();
        private LoadCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment keyType,java.lang.foreign.MemorySegment keyId,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.get(uniffiHandle);
            java.util.concurrent.Callable<java.util.List<java.lang.Byte>> _uniffiMakeCall = () -> {
                return _uniffiObj.load(
                    FfiConverterString.INSTANCE.lift(keyType),
                    FfiConverterString.INSTANCE.lift(keyId)
                );
                
            };
            java.util.function.Consumer<java.util.List<java.lang.Byte>> _uniffiWriteReturn = (java.util.List<java.lang.Byte> _uniffiValue) -> {
                java.lang.foreign.MemorySegment _uniffiOut = uniffiOutReturn.reinterpret(RustBuffer.LAYOUT.byteSize());
                java.lang.foreign.MemorySegment _uniffiLowered = FfiConverterOptionalSequenceByte.INSTANCE.lower(_uniffiValue);
                java.lang.foreign.MemorySegment.copy(_uniffiLowered, 0, _uniffiOut, 0, RustBuffer.LAYOUT.byteSize());
            };
            UniffiHelpers.uniffiTraitInterfaceCallWithError(
                _uniffiCallStatus,
                _uniffiMakeCall,
                _uniffiWriteReturn,
                (MlsStorageException _uniffiErr) -> { return FfiConverterTypeMlsStorageError.INSTANCE.lower(_uniffiErr); },
                MlsStorageException.class
            );
        }
    }
    
    public static final class DeleteCallback implements UniffiCallbackInterfaceMlsStorageProviderMethod2.Fn {
        public static final DeleteCallback INSTANCE = new DeleteCallback();
        private DeleteCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment keyType,java.lang.foreign.MemorySegment keyId,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.get(uniffiHandle);
            java.util.concurrent.Callable<java.lang.Void> _uniffiMakeCall = () -> {
                _uniffiObj.delete(
                    FfiConverterString.INSTANCE.lift(keyType),
                    FfiConverterString.INSTANCE.lift(keyId)
                );
                return null;
            };
            java.util.function.Consumer<java.lang.Void> _uniffiWriteReturn = (_uniffiNothing) -> {};
            UniffiHelpers.uniffiTraitInterfaceCallWithError(
                _uniffiCallStatus,
                _uniffiMakeCall,
                _uniffiWriteReturn,
                (MlsStorageException _uniffiErr) -> { return FfiConverterTypeMlsStorageError.INSTANCE.lower(_uniffiErr); },
                MlsStorageException.class
            );
        }
    }
    
    public static final class ListKeysCallback implements UniffiCallbackInterfaceMlsStorageProviderMethod3.Fn {
        public static final ListKeysCallback INSTANCE = new ListKeysCallback();
        private ListKeysCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment keyType,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.get(uniffiHandle);
            java.util.concurrent.Callable<java.util.List<java.lang.String>> _uniffiMakeCall = () -> {
                return _uniffiObj.listKeys(
                    FfiConverterString.INSTANCE.lift(keyType)
                );
                
            };
            java.util.function.Consumer<java.util.List<java.lang.String>> _uniffiWriteReturn = (java.util.List<java.lang.String> _uniffiValue) -> {
                java.lang.foreign.MemorySegment _uniffiOut = uniffiOutReturn.reinterpret(RustBuffer.LAYOUT.byteSize());
                java.lang.foreign.MemorySegment _uniffiLowered = FfiConverterSequenceString.INSTANCE.lower(_uniffiValue);
                java.lang.foreign.MemorySegment.copy(_uniffiLowered, 0, _uniffiOut, 0, RustBuffer.LAYOUT.byteSize());
            };
            UniffiHelpers.uniffiTraitInterfaceCallWithError(
                _uniffiCallStatus,
                _uniffiMakeCall,
                _uniffiWriteReturn,
                (MlsStorageException _uniffiErr) -> { return FfiConverterTypeMlsStorageError.INSTANCE.lower(_uniffiErr); },
                MlsStorageException.class
            );
        }
    }

    public static class UniffiFree implements UniffiCallbackInterfaceFree.Fn {
        public static final UniffiFree INSTANCE = new UniffiFree();

        private UniffiFree() {}

        @Override
        public void callback(long handle) {
            FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.remove(handle);
        }
    }

    public static class UniffiClone implements UniffiCallbackInterfaceClone.Fn {
        public static final UniffiClone INSTANCE = new UniffiClone();

        private UniffiClone() {}

        @Override
        public long callback(long handle) {
            return FfiConverterTypeMlsStorageProvider.INSTANCE.handleMap.clone(handle);
        }
    }
}

