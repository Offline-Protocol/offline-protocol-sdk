package com.offlineprotocol.sdk;


// Put the implementation in an object so we don't pollute the top-level namespace
public class UniffiCallbackInterfaceBleTransportCallback {
    public static final UniffiCallbackInterfaceBleTransportCallback INSTANCE = new UniffiCallbackInterfaceBleTransportCallback();
    java.lang.foreign.MemorySegment vtable;

    UniffiCallbackInterfaceBleTransportCallback() {
        // Use Arena.global() for vtable and upcall stubs - they live for the program lifetime.
        // Arena.ofAuto() stubs can be GC'd since storing an address in a struct doesn't
        // prevent the Arena from being collected.
        vtable = java.lang.foreign.Arena.global().allocate(UniffiVTableCallbackInterfaceBleTransportCallback.LAYOUT);
        UniffiVTableCallbackInterfaceBleTransportCallback.setuniffiFree(vtable, UniffiCallbackInterfaceFree.toUpcallStub(UniffiFree.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceBleTransportCallback.setuniffiClone(vtable, UniffiCallbackInterfaceClone.toUpcallStub(UniffiClone.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceBleTransportCallback.setonFragmentsAvailable(vtable, UniffiCallbackInterfaceBleTransportCallbackMethod0.toUpcallStub(OnFragmentsAvailableCallback.INSTANCE, java.lang.foreign.Arena.global()));
    }

    // Registers the foreign callback with the Rust side.
    void register() {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_init_callback_vtable_bletransportcallback(vtable);
    }
    
    public static final class OnFragmentsAvailableCallback implements UniffiCallbackInterfaceBleTransportCallbackMethod0.Fn {
        public static final OnFragmentsAvailableCallback INSTANCE = new OnFragmentsAvailableCallback();
        private OnFragmentsAvailableCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeBleTransportCallback.INSTANCE.handleMap.get(uniffiHandle);
            java.util.function.Supplier<java.lang.Void> _uniffiMakeCall = () -> {
                _uniffiObj.onFragmentsAvailable(
                );
                return null;
            };
            java.util.function.Consumer<java.lang.Void> _uniffiWriteReturn = (_uniffiNothing) -> {};
            UniffiHelpers.uniffiTraitInterfaceCall(_uniffiCallStatus, _uniffiMakeCall, _uniffiWriteReturn);
        }
    }

    public static class UniffiFree implements UniffiCallbackInterfaceFree.Fn {
        public static final UniffiFree INSTANCE = new UniffiFree();

        private UniffiFree() {}

        @Override
        public void callback(long handle) {
            FfiConverterTypeBleTransportCallback.INSTANCE.handleMap.remove(handle);
        }
    }

    public static class UniffiClone implements UniffiCallbackInterfaceClone.Fn {
        public static final UniffiClone INSTANCE = new UniffiClone();

        private UniffiClone() {}

        @Override
        public long callback(long handle) {
            return FfiConverterTypeBleTransportCallback.INSTANCE.handleMap.clone(handle);
        }
    }
}

