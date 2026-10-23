package com.offlineprotocol.sdk;


// The cleaner interface for Object finalization code to run.
// Uses a custom PhantomReference-based implementation that provides backpressure:
// when objects are created faster than the background cleaner thread can process them,
// the registering thread drains pending cleanups inline, preventing OOM in
// high-throughput scenarios (e.g., benchmarks, tight loops without explicit close()).
interface UniffiCleaner {
    interface Cleanable {
        void clean();
    }

    UniffiCleaner.Cleanable register(java.lang.Object value, java.lang.Runnable cleanUpTask);

    public static UniffiCleaner create() {
        return new UniffiBackpressureCleaner();
    }
}

