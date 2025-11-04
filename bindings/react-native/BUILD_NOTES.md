# Build Notes for npm Publishing

## Required Build Artifacts

Before publishing to npm, ensure the following Rust FFI libraries are built and placed in the correct locations:

### Android Libraries

Build for all Android architectures and place in:
```
android/src/main/jniLibs/
├── arm64-v8a/liboffline_protocol_ffi.so
├── armeabi-v7a/liboffline_protocol_ffi.so
├── x86_64/liboffline_protocol_ffi.so
└── x86/liboffline_protocol_ffi.so
```

### iOS Libraries

Build universal library and place in:
```
ios/
├── liboffline_protocol_ffi.a
└── offline_protocol.h
```

### Android Header File

The header file should also be copied to:
```
android/src/main/cpp/offline_protocol.h
```

This is needed for the JNI wrapper compilation.

## Build Process

The `prepublishOnly` script will automatically:
1. Build TypeScript (`npm run build`)
2. Build Rust libraries for all platforms (`npm run build:rust`)

Make sure you have:
- Rust toolchain installed
- Android NDK set up (for Android builds)
- Xcode installed (for iOS builds)

## Verification

After building, verify all files exist:

```bash
# Check Android libraries
ls -la android/src/main/jniLibs/*/liboffline_protocol_ffi.so

# Check iOS library
ls -la ios/liboffline_protocol_ffi.a
ls -la ios/offline_protocol.h

# Check Android header
ls -la android/src/main/cpp/offline_protocol.h
```

## Package Size

Including pre-built Rust libraries increases package size but provides:
- ✅ No Rust toolchain required for users
- ✅ Faster installation
- ✅ Better user experience
- ✅ Works out of the box

