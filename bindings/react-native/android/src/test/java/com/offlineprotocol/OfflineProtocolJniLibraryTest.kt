package com.offlineprotocol

import org.junit.Assert.assertEquals
import org.junit.Test

class OfflineProtocolJniLibraryTest {
    @Test
    fun namesMatchWhatUniFFILoadsAndCargoWrites() {
        assertEquals("libuniffi_offline_protocol.so", OfflineProtocolJniLibrary.LOADED_BASENAME)
        assertEquals("liboffline_protocol_uniffi.so", OfflineProtocolJniLibrary.CARGO_OUTPUT_BASENAME)
    }
}
