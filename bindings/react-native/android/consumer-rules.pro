# Consumer ProGuard rules for @offline-protocol/mesh-sdk.
# Applied automatically to any app that consumes this library and minifies
# (R8 / ProGuard). Without these, release builds (TestFlight / Play Store)
# silently lose the BLE mesh transport while debug/Metro builds work.
#
# The Rust core is reached from Kotlin via JNA, and the uniffi-generated
# bindings map Kotlin <-> native ENTIRELY BY REFLECTION over class / field /
# method names: Native.register direct mapping, @Structure.FieldOrder
# vtables, and com.sun.jna.Callback interfaces for the Rust -> Kotlin
# callbacks (onFragmentsAvailable, blePeerDiscovered, ...). R8 obfuscation
# renames them and the FFI dies at runtime.

# SDK Kotlin (RN module, BleManager, TransportManager, MlsSecureStorage).
-keep class com.offlineprotocol.** { *; }

# uniffi-generated FFI bindings (separate package from com.offlineprotocol).
-keep class uniffi.** { *; }
-keepclassmembers class uniffi.** { *; }
-keep interface uniffi.** { *; }

# JNA — resolves native symbols, Structure fields, and Callback methods by
# name at runtime; obfuscating them breaks the FFI.
-keep class com.sun.jna.** { *; }
-keepclassmembers class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.Structure { <fields>; }
-keep class * implements com.sun.jna.Callback { *; }

# JNA reads @Structure.FieldOrder and resolves nested Structure.ByValue /
# ByReference types at runtime (no getFieldOrder() fallback), so the
# annotation + inner-class metadata must survive R8 (incl. full mode).
-keepattributes *Annotation*, Signature, InnerClasses, EnclosingMethod

# JNA references java.awt, which does not exist on Android.
-dontwarn java.awt.**
