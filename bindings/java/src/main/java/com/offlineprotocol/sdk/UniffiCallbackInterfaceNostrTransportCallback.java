package com.offlineprotocol.sdk;


// Put the implementation in an object so we don't pollute the top-level namespace
public class UniffiCallbackInterfaceNostrTransportCallback {
    public static final UniffiCallbackInterfaceNostrTransportCallback INSTANCE = new UniffiCallbackInterfaceNostrTransportCallback();
    java.lang.foreign.MemorySegment vtable;

    UniffiCallbackInterfaceNostrTransportCallback() {
        // Use Arena.global() for vtable and upcall stubs - they live for the program lifetime.
        // Arena.ofAuto() stubs can be GC'd since storing an address in a struct doesn't
        // prevent the Arena from being collected.
        vtable = java.lang.foreign.Arena.global().allocate(UniffiVTableCallbackInterfaceNostrTransportCallback.LAYOUT);
        UniffiVTableCallbackInterfaceNostrTransportCallback.setuniffiFree(vtable, UniffiCallbackInterfaceFree.toUpcallStub(UniffiFree.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceNostrTransportCallback.setuniffiClone(vtable, UniffiCallbackInterfaceClone.toUpcallStub(UniffiClone.INSTANCE, java.lang.foreign.Arena.global()));
        UniffiVTableCallbackInterfaceNostrTransportCallback.setonMessagesAvailable(vtable, UniffiCallbackInterfaceNostrTransportCallbackMethod0.toUpcallStub(OnMessagesAvailableCallback.INSTANCE, java.lang.foreign.Arena.global()));
    }

    // Registers the foreign callback with the Rust side.
    void register() {
        UniffiLib.uniffi_offline_protocol_uniffi_fn_init_callback_vtable_nostrtransportcallback(vtable);
    }
    
    public static final class OnMessagesAvailableCallback implements UniffiCallbackInterfaceNostrTransportCallbackMethod0.Fn {
        public static final OnMessagesAvailableCallback INSTANCE = new OnMessagesAvailableCallback();
        private OnMessagesAvailableCallback() {}

        @Override
        public void callback(long uniffiHandle,java.lang.foreign.MemorySegment uniffiOutReturn,java.lang.foreign.MemorySegment _uniffiCallStatus) {
            _uniffiCallStatus = _uniffiCallStatus.reinterpret(UniffiRustCallStatus.LAYOUT.byteSize());
            var _uniffiObj = FfiConverterTypeNostrTransportCallback.INSTANCE.handleMap.get(uniffiHandle);
            java.util.function.Supplier<java.lang.Void> _uniffiMakeCall = () -> {
                _uniffiObj.onMessagesAvailable(
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
            FfiConverterTypeNostrTransportCallback.INSTANCE.handleMap.remove(handle);
        }
    }

    public static class UniffiClone implements UniffiCallbackInterfaceClone.Fn {
        public static final UniffiClone INSTANCE = new UniffiClone();

        private UniffiClone() {}

        @Override
        public long callback(long handle) {
            return FfiConverterTypeNostrTransportCallback.INSTANCE.handleMap.clone(handle);
        }
    }
}

