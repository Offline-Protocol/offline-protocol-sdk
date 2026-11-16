package com.offlineprotocol

/**
 * JNI file names for manual integration (copying `.so` files into `jniLibs`).
 *
 * Cargo links `liboffline_protocol_uniffi.so`. UniFFI's Kotlin loader opens
 * [LOADED_BASENAME] only. See docs/android-jni-and-imports.md.
 */
object OfflineProtocolJniLibrary {
    /** The file name every ABI folder under `jniLibs/` must use. */
    const val LOADED_BASENAME: String = "libuniffi_offline_protocol.so"

    /** The file name `cargo build` writes before you rename on copy. */
    const val CARGO_OUTPUT_BASENAME: String = "liboffline_protocol_uniffi.so"
}
