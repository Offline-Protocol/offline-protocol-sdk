package com.offlineprotocol.sdk;


// Helpers for calling Rust
// In practice we usually need to be synchronized to call this safely, so it doesn't
// synchronize itself
public final class UniffiHelpers {
    // Thread-local reusable RustCallStatus to avoid allocation on the hot path
    private static final ThreadLocal<java.lang.foreign.MemorySegment> REUSABLE_STATUS = ThreadLocal.withInitial(() ->
        java.lang.foreign.Arena.global().allocate(UniffiRustCallStatus.LAYOUT)
    );

    // Slab allocator for struct return values from FFI downcalls.
    // See UniffiSlabAllocator for the design rationale.
    private static final UniffiSlabAllocator RETURN_ALLOCATOR = new UniffiSlabAllocator(RustBuffer.LAYOUT, 1024);


    @FunctionalInterface
    public interface UniffiRustCallFunction<U> {
        U apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    @FunctionalInterface
    public interface UniffiRustCallVoidFunction {
        void apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    // Call a rust function that returns a Result<>. Pass in the Error class companion that
    // corresponds to the Err.
    static <U, E extends java.lang.Exception> U uniffiRustCallWithError(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallFunction<U> callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        U returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    // Overload for void-returning functions
    static <E extends java.lang.Exception> void uniffiRustCallWithError(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallVoidFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
    }

    // Check UniffiRustCallStatus and throw an error if the call wasn't successful
    static <E extends java.lang.Exception> void uniffiCheckCallStatus(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            java.lang.foreign.MemorySegment status) throws E {
        if (UniffiRustCallStatus.isSuccess(status)) {
            return;
        } else if (UniffiRustCallStatus.isError(status)) {
            throw errorHandler.lift(UniffiRustCallStatus.getErrorBuf(status));
        } else if (UniffiRustCallStatus.isPanic(status)) {
            java.lang.foreign.MemorySegment errorBuf = UniffiRustCallStatus.getErrorBuf(status);
            if (RustBuffer.getLen(errorBuf) > 0) {
                throw new InternalException(FfiConverterString.INSTANCE.lift(errorBuf));
            } else {
                throw new InternalException("Rust panic");
            }
        } else {
            throw new InternalException("Unknown rust call status: " + UniffiRustCallStatus.getCode(status));
        }
    }

    // Primitive-specialized variants that avoid autoboxing overhead.
    // For each primitive type, we have a functional interface + call + callWithError.

    @FunctionalInterface
    interface UniffiRustCallLongFunction {
        long apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> long uniffiRustCallWithErrorLong(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallLongFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        long returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static long uniffiRustCallLong(UniffiRustCallLongFunction callback) {
        return uniffiRustCallWithErrorLong(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallIntFunction {
        int apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> int uniffiRustCallWithErrorInt(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallIntFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        int returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static int uniffiRustCallInt(UniffiRustCallIntFunction callback) {
        return uniffiRustCallWithErrorInt(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallShortFunction {
        short apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> short uniffiRustCallWithErrorShort(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallShortFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        short returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static short uniffiRustCallShort(UniffiRustCallShortFunction callback) {
        return uniffiRustCallWithErrorShort(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallByteFunction {
        byte apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> byte uniffiRustCallWithErrorByte(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallByteFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        byte returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static byte uniffiRustCallByte(UniffiRustCallByteFunction callback) {
        return uniffiRustCallWithErrorByte(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallFloatFunction {
        float apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> float uniffiRustCallWithErrorFloat(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallFloatFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        float returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static float uniffiRustCallFloat(UniffiRustCallFloatFunction callback) {
        return uniffiRustCallWithErrorFloat(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallDoubleFunction {
        double apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> double uniffiRustCallWithErrorDouble(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallDoubleFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        double returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static double uniffiRustCallDouble(UniffiRustCallDoubleFunction callback) {
        return uniffiRustCallWithErrorDouble(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    @FunctionalInterface
    interface UniffiRustCallBooleanFunction {
        boolean apply(java.lang.foreign.SegmentAllocator allocator, java.lang.foreign.MemorySegment status);
    }

    static <E extends java.lang.Exception> boolean uniffiRustCallWithErrorBoolean(
            UniffiRustCallStatusErrorHandler<E> errorHandler,
            UniffiRustCallBooleanFunction callback) throws E {
        java.lang.foreign.MemorySegment status = REUSABLE_STATUS.get();
        status.fill((byte) 0);
        boolean returnValue = callback.apply(RETURN_ALLOCATOR, status);
        uniffiCheckCallStatus(errorHandler, status);
        return returnValue;
    }

    static boolean uniffiRustCallBoolean(UniffiRustCallBooleanFunction callback) {
        return uniffiRustCallWithErrorBoolean(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    // Call a rust function that returns a plain value
    static <U> U uniffiRustCall(UniffiRustCallFunction<U> callback) {
        return uniffiRustCallWithError(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    // Call a rust function that returns nothing
    static void uniffiRustCall(UniffiRustCallVoidFunction callback) {
        uniffiRustCallWithError(new UniffiNullRustCallStatusErrorHandler(), callback);
    }

    static <T> void uniffiTraitInterfaceCall(
        java.lang.foreign.MemorySegment callStatus,
        java.util.function.Supplier<T> makeCall,
        java.util.function.Consumer<T> writeReturn
    ) {
        try {
            writeReturn.accept(makeCall.get());
        } catch (java.lang.Exception e) {
            UniffiRustCallStatus.setCode(callStatus, UniffiRustCallStatus.UNIFFI_CALL_UNEXPECTED_ERROR);
            UniffiRustCallStatus.setErrorBuf(callStatus, FfiConverterString.INSTANCE.lower(uniffiStackTraceToString(e)));
        }
    }

    private static java.lang.String uniffiStackTraceToString(java.lang.Throwable e) {
        try {
            java.io.StringWriter sw = new java.io.StringWriter();
            e.printStackTrace(new java.io.PrintWriter(sw));
            return sw.toString();
        } catch (java.lang.Throwable _t) {
            return e.toString();
        }
    }

    static <T, E extends java.lang.Throwable> void uniffiTraitInterfaceCallWithError(
        java.lang.foreign.MemorySegment callStatus,
        java.util.concurrent.Callable<T> makeCall,
        java.util.function.Consumer<T> writeReturn,
        java.util.function.Function<E, java.lang.foreign.MemorySegment> lowerError,
        java.lang.Class<E> errorClazz
    ) {
        try {
            writeReturn.accept(makeCall.call());
        } catch (java.lang.Exception e) {
            if (errorClazz.isAssignableFrom(e.getClass())) {
                @SuppressWarnings("unchecked")
                E castedE = (E) e;
                UniffiRustCallStatus.setCode(callStatus, UniffiRustCallStatus.UNIFFI_CALL_ERROR);
                UniffiRustCallStatus.setErrorBuf(callStatus, lowerError.apply(castedE));
            } else {
                UniffiRustCallStatus.setCode(callStatus, UniffiRustCallStatus.UNIFFI_CALL_UNEXPECTED_ERROR);
                UniffiRustCallStatus.setErrorBuf(callStatus, FfiConverterString.INSTANCE.lower(uniffiStackTraceToString(e)));
            }
        }
    }
}

