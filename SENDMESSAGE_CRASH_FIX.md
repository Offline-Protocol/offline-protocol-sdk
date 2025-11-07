# SendMessage Crash Fix

## Problem
The app was crashing on both iOS and Android when calling `sendMessage`. The crash occurred in the native JNI layer during `nativeSendMessage` execution.

## Root Causes Identified

### 1. Thread Safety Issue (CRITICAL)
**Location:** `crates/offline-protocol-ffi/src/lib.rs`

**Problem:** All FFI functions were creating mutable references to the `ProtocolHandle`:
```rust
let handle_ref = &mut *handle;
```

This is **undefined behavior** when multiple threads access the same handle, which is exactly what happens in a React Native app where:
- The UI thread calls `sendMessage`
- Background threads process BLE fragments
- Event callbacks fire on different threads

**Fix:** Changed to immutable references since `ProtocolHandle` uses interior mutability:
```rust
let handle_ref = &*handle;
```

All fields in `ProtocolHandle` are wrapped in `Mutex` or `Arc<Mutex<...>>`, so interior mutability is the correct pattern.

**Files Fixed:**
- `crates/offline-protocol-ffi/src/lib.rs` - 12 instances of `&mut *handle` changed to `&*handle`

### 2. Uninitialized Buffer (DEFENSIVE)
**Location:** Multiple FFI functions

**Problem:** Output buffers were not initialized, which could cause crashes if accessed on error paths or if buffer initialization failed.

**Fix:** 
- In Rust FFI: Initialize output buffer to null-terminated empty string before processing
- In JNI (C++): Use `memset` to zero-initialize buffers
- In iOS (Swift): Use `buffer.initialize(repeating: 0, count: 256)`

**Files Fixed:**
- `crates/offline-protocol-ffi/src/lib.rs` - Added buffer initialization in `offline_protocol_send_message`
- `bindings/react-native/android/src/main/cpp/offline_protocol_jni.cpp` - Added `memset` and error checking
- `bindings/react-native/ios/OfflineProtocolModule.swift` - Added buffer initialization

### 3. Missing Error Checking in JNI (DEFENSIVE)
**Location:** `bindings/react-native/android/src/main/cpp/offline_protocol_jni.cpp`

**Problem:** JNI string conversions were not checked for null returns, which could cause crashes if memory allocation failed.

**Fix:** Added comprehensive error checking:
```cpp
const char* recipientStr = env->GetStringUTFChars(recipient, nullptr);
if (recipientStr == nullptr) {
    LOGE("Failed to get recipient string");
    return nullptr;
}
```

Also added error logging for all failure paths.

## Testing Recommendations

1. **Multi-threaded Testing:** Send multiple messages rapidly from different threads
2. **Stress Testing:** Send many messages while BLE connections are being established
3. **Background Mode:** Test sending messages when app goes to background and foreground
4. **Connection Churn:** Test with peers connecting and disconnecting frequently

## Files Modified

1. `crates/offline-protocol-ffi/src/lib.rs` - Thread safety and buffer initialization
2. `bindings/react-native/android/src/main/cpp/offline_protocol_jni.cpp` - Error checking and buffer initialization
3. `bindings/react-native/ios/OfflineProtocolModule.swift` - Buffer initialization

## Building and Testing

To rebuild the native libraries:

### Android
```bash
cd examples/react-native-app/android
./gradlew assembleDebug
```

### iOS
```bash
cd examples/react-native-app/ios
pod install
xcodebuild -workspace OfflineProtocolExample.xcworkspace -scheme OfflineProtocolExample
```

## Impact

This fix addresses:
- ✅ Crash on sendMessage (both platforms)
- ✅ Thread safety issues in FFI layer
- ✅ Potential memory corruption from uninitialized buffers
- ✅ Better error messages for debugging

## Note

The thread safety issue (`&mut *handle`) was the most critical bug. This could cause:
- Data races
- Memory corruption
- Unpredictable crashes
- Especially problematic on multi-core devices

The fix is essential for production use.

