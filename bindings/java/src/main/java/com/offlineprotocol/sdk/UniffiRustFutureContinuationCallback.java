package com.offlineprotocol.sdk;


public final class UniffiRustFutureContinuationCallback {
    public static final java.lang.foreign.FunctionDescriptor DESCRIPTOR = java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, java.lang.foreign.ValueLayout.JAVA_BYTE);

    @FunctionalInterface
    public interface Fn {
        void callback(long data,byte pollResult);
    }

    public static java.lang.foreign.MemorySegment toUpcallStub(Fn fn, java.lang.foreign.Arena arena) {
        try {
            java.lang.invoke.MethodHandle handle = java.lang.invoke.MethodHandles.lookup()
                .findVirtual(Fn.class, "callback", java.lang.invoke.MethodType.methodType(
                    void.class,
                    long.class,
                    byte.class
                ))
                .bindTo(fn);
            return java.lang.foreign.Linker.nativeLinker().upcallStub(handle, DESCRIPTOR, arena);
        } catch (NoSuchMethodException | IllegalAccessException e) {
            throw new AssertionError("Failed to create upcall stub", e);
        }
    }
}
