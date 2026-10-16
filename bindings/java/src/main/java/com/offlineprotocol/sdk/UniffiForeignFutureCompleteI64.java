package com.offlineprotocol.sdk;


public final class UniffiForeignFutureCompleteI64 {
    public static final java.lang.foreign.FunctionDescriptor DESCRIPTOR = java.lang.foreign.FunctionDescriptor.ofVoid(java.lang.foreign.ValueLayout.JAVA_LONG, UniffiForeignFutureResultI64.LAYOUT);

    @FunctionalInterface
    public interface Fn {
        void callback(long callbackData,java.lang.foreign.MemorySegment result);
    }

    public static java.lang.foreign.MemorySegment toUpcallStub(Fn fn, java.lang.foreign.Arena arena) {
        try {
            java.lang.invoke.MethodHandle handle = java.lang.invoke.MethodHandles.lookup()
                .findVirtual(Fn.class, "callback", java.lang.invoke.MethodType.methodType(
                    void.class,
                    long.class,
                    java.lang.foreign.MemorySegment.class
                ))
                .bindTo(fn);
            return java.lang.foreign.Linker.nativeLinker().upcallStub(handle, DESCRIPTOR, arena);
        } catch (NoSuchMethodException | IllegalAccessException e) {
            throw new AssertionError("Failed to create upcall stub", e);
        }
    }
}
