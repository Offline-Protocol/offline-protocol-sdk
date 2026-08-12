# Wiring Guide: Rust to React Native

How to expose a new Rust function to React Native across iOS and Android. Every layer must be updated — a missing step means the method silently doesn't exist on one platform.

---

## Architecture Overview

```
Rust (core logic)
  ↓
UDL definition (offline_protocol.udl)
  ↓
UniFFI code generation (auto-generated Swift + Kotlin wrappers)
  ↓
React Native native modules (hand-written platform bridges)
  ├── iOS:  OfflineProtocolModule.swift  (@objc implementation)
  │         OfflineProtocolModule.m      (RCT_EXTERN_METHOD declarations)
  └── Android: OfflineProtocolModule.kt (@ReactMethod implementations)
  ↓
TypeScript API (src/index.ts)
```

---

## Step-by-Step: Adding a New Method

### 1. Rust Implementation

Add the method to the protocol in the appropriate crate (e.g., `offline-protocol`), then expose it through the UniFFI bridge crate.

**File:** `crates/offline-protocol-uniffi/src/lib.rs`

```rust
impl OfflineProtocol {
    pub fn do_something(&self, param: String) -> Result<String, ProtocolError> {
        self.inner.lock().unwrap().do_something(param)
    }
}
```

### 2. UDL Definition

Declare the method in the UniFFI interface definition. This is the **single source of truth** for cross-platform bindings.

**File:** `crates/offline-protocol-uniffi/src/offline_protocol.udl`

```idl
interface OfflineProtocol {
    // ... existing methods ...
    [Throws=ProtocolError]
    string do_something(string param);
};
```

### 3. Regenerate Bindings

```bash
./scripts/generate-bindings.sh          # or: npm run generate:bindings
```

This regenerates Swift, Kotlin **and** Python in one pass — they are one
artifact set off one UDL, so they are never refreshed apart. Commit all three.

This auto-generates:
- `ios/Generated/offline_protocol.swift` — Swift wrapper classes
- `ios/Generated/offline_protocolFFI.h` — C header for FFI
- `android/src/main/java/uniffi/offline_protocol/offline_protocol.kt` — Kotlin wrapper classes

**Do not hand-edit generated files.**

### 4. iOS Native Module (TWO files — this is where things break)

#### 4a. Swift Implementation

**File:** `ios/OfflineProtocolModule.swift`

```swift
@objc func doSomething(_ param: String,
                       resolver resolve: @escaping RCTPromiseResolveBlock,
                       rejecter reject: @escaping RCTPromiseRejectBlock) {
    do {
        guard let proto = protocolInstance else {
            throw NSError(domain: "OfflineProtocol", code: -1,
                        userInfo: [NSLocalizedDescriptionKey: "Protocol not initialized"])
        }
        let result = try proto.doSomething(param: param)
        resolve(result)
    } catch {
        reject("ERROR_DO_SOMETHING", error.localizedDescription, error)
    }
}
```

#### 4b. Objective-C Bridge Declarations (THE STEP THAT GETS MISSED)

**File:** `ios/OfflineProtocolModule.m`

```objc
RCT_EXTERN_METHOD(doSomething:(NSString *)param
                  resolver:(RCTPromiseResolveBlock)resolve
                  rejecter:(RCTPromiseRejectBlock)reject)
```

**Why this exists:** React Native on iOS uses an Objective-C bridge to discover native methods. The `.swift` file has the implementation, but the `.m` file has the **registration**. If you add an `@objc func` in Swift without a matching `RCT_EXTERN_METHOD` in the `.m` file, the method compiles fine but **JavaScript cannot call it**. There is no build error — it fails silently at runtime.

**Parameter type mapping for RCT_EXTERN_METHOD:**

| Swift Type | Obj-C Bridge Type |
|------------|-------------------|
| `String` | `NSString *` |
| `Int` / `NSNumber` | `nonnull NSNumber *` |
| `Bool` | `BOOL` |
| `[Any]` / `NSArray` | `NSArray *` |
| `[String: Any]` / `NSDictionary` | `NSDictionary *` |
| `String?` (optional) | `NSString *` |
| Promise resolve | `RCTPromiseResolveBlock` |
| Promise reject | `RCTPromiseRejectBlock` |

### 5. Android Native Module (one file)

**File:** `android/src/main/java/com/offlineprotocol/OfflineProtocolModule.kt`

```kotlin
@ReactMethod
fun doSomething(param: String, promise: Promise) {
    try {
        val proto = protocol ?: throw IllegalStateException("Protocol not initialized")
        val result = proto.doSomething(param)
        promise.resolve(result)
    } catch (e: Exception) {
        val mapped = mapProtocolBridgeError(e)
        if (mapped != null) {
            promise.reject(mapped.code, mapped.message, e)
        } else {
            promise.reject("ERROR_DO_SOMETHING", "Failed: ${e.message}", e)
        }
    }
}
```

On Android, `@ReactMethod` handles both declaration and registration — there is no separate bridge file. This is why iOS-only bugs are common.

### 6. TypeScript API

**File:** `src/index.ts`

```typescript
async doSomething(param: string): Promise<string> {
    return await OfflineProtocolNativeModule.doSomething(param);
}
```

Add corresponding types in `src/types.ts` if needed.

---

## Checklist

Use this checklist every time you add or modify a bridged method:

```
[ ] Rust implementation in uniffi crate (lib.rs)
[ ] UDL declaration (offline_protocol.udl)
[ ] Regenerated bindings (npm run generate:bindings)
[ ] iOS: @objc func in OfflineProtocolModule.swift
[ ] iOS: RCT_EXTERN_METHOD in OfflineProtocolModule.m  ← EASY TO FORGET
[ ] iOS: Parameter types in .m match the Swift signature exactly
[ ] Android: @ReactMethod in OfflineProtocolModule.kt
[ ] TypeScript: Method + types in src/index.ts and src/types.ts
```

---

## Common Mistakes

### 1. Missing RCT_EXTERN_METHOD (iOS)

**Symptom:** Method works on Android but `undefined` or "not a function" on iOS.
**Cause:** Swift `@objc func` exists but no `RCT_EXTERN_METHOD` in `.m` file.
**Fix:** Add the macro to `OfflineProtocolModule.m`.

### 2. Parameter Name Mismatch (iOS)

**Symptom:** iOS crash or wrong argument values.
**Cause:** The parameter names in `RCT_EXTERN_METHOD` must match the Swift function's **external** parameter names exactly. Swift uses the first argument label differently from subsequent ones.
**Example:**
```swift
// Swift: first param has no external label (uses _), second uses "status:"
@objc func sendPresenceUpdate(_ recipient: String,
                              status: NSNumber, ...)
```
```objc
// .m file: first param name matches, second uses the external label
RCT_EXTERN_METHOD(sendPresenceUpdate:(NSString *)recipient
                  status:(nonnull NSNumber *)status ...)
```

### 3. Wrong Number Type (iOS)

**Symptom:** `nil` value received for numeric parameters.
**Cause:** Objective-C bridge requires `nonnull NSNumber *` for numbers, not primitive `int`. Use `BOOL` for booleans.

### 4. Forgetting to Update One Platform

**Symptom:** Feature works on one platform, not the other.
**Cause:** Updated Android but forgot iOS, or vice versa.
**Fix:** Always update both platforms together. Use the checklist above.

---

## Verification

After wiring a new method, verify it exists on both platforms:

```typescript
// Quick smoke test in your RN app
import { NativeModules } from 'react-native';
console.log(typeof NativeModules.OfflineProtocolModule.doSomething);
// Should log "function" on both iOS and Android
// If "undefined" on iOS → missing RCT_EXTERN_METHOD
```

---

## File Reference

| File | Role | Hand-written? |
|------|------|---------------|
| `crates/offline-protocol-uniffi/src/lib.rs` | Rust FFI implementation | Yes |
| `crates/offline-protocol-uniffi/src/offline_protocol.udl` | API definition (source of truth) | Yes |
| `ios/Generated/offline_protocol.swift` | Swift bindings | **No** (auto-generated) |
| `ios/Generated/offline_protocolFFI.h` | C FFI header | **No** (auto-generated) |
| `ios/OfflineProtocolModule.swift` | iOS RN bridge (implementation) | Yes |
| `ios/OfflineProtocolModule.m` | iOS RN bridge (method registration) | Yes |
| `android/.../uniffi/offline_protocol/offline_protocol.kt` | Kotlin bindings | **No** (auto-generated) |
| `android/.../OfflineProtocolModule.kt` | Android RN bridge | Yes |
| `src/index.ts` | TypeScript API | Yes |
| `src/types.ts` | TypeScript types | Yes |
