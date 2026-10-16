package com.offlineprotocol.sdk;


/**
 * Ensures the native library is initialized.
 * Call this function to force initialization before using any types from this library.
 */
public final class UniffiInitializer {
    /**
     * Force initialization of the native library.
     */
    public static void ensureInitialized() {
        // Force UniffiLib class to load (runs integrity checks and initialization functions)
        Class<?> ignored = UniffiLib.class;
    }
}

// Async support

// Public interface members begin here.


