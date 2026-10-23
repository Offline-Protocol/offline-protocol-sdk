package com.offlineprotocol.sdk;


// Marker class for constructors that accept a raw handle.
// This disambiguates constructor signatures when an interface has both
// a regular constructor and one accepting an FFI handle.
public class UniffiWithHandle {
    // Private constructor to prevent instantiation
    private UniffiWithHandle() {}

    public static final UniffiWithHandle INSTANCE = new UniffiWithHandle();
}
