# Java and C# on desktop: packages and native libraries

Friction **#23**: desktop hosts should not need a Rust toolchain for the
**blessed integrator path**. The Java jar and the C# binding ship **without**
embedding the native library; pair them with **release desktop archives** (same
binaries as Python wheels).

For the shared C ABI and JSON calling style, see [bindings/c/README.md](../bindings/c/README.md).

## Choose your path

| Language | API package | Native library |
|----------|-------------|----------------|
| **Java** (JDK 22+) | Maven `com.offlineprotocol:offline-protocol-java` | GitHub release `offline-protocol-X.Y.Z-<platform>.tar.gz` or `.zip` |
| **C#** (.NET 8+) | Build from repo or vendor `OfflineProtocol.Sdk` | Same release archive + `OFFLINE_PROTOCOL_LIBRARY_DIR` |
| **C / others** | Header in repo + release archive | Same archives ([bridges/c.md](bridges/c.md)) |

There is **no NuGet** package yet: C# integrators use release assets or a local
build. There is **no Maven artifact that bundles** the `.so` / `.dylib` / `.dll`:
the JVM loads natives from your disk (`java.library.path` or
`libraryOverride`).

## Step 1 — Pick the release native archive

From [GitHub releases](https://github.com/Offline-Protocol/offline-protocol-sdk/releases)
for version `vX.Y.Z`:

| Platform | Asset | Library inside |
|----------|--------|----------------|
| Linux x86_64 | `offline-protocol-X.Y.Z-linux-x86_64.tar.gz` | `liboffline_protocol_uniffi.so` |
| Linux aarch64 | `offline-protocol-X.Y.Z-linux-aarch64.tar.gz` | `liboffline_protocol_uniffi.so` |
| macOS arm64 | `offline-protocol-X.Y.Z-macos-arm64.tar.gz` | `liboffline_protocol_uniffi.dylib` |
| Windows x86_64 | `offline-protocol-X.Y.Z-windows-x86_64.zip` | `offline_protocol_uniffi.dll` (+ `.dll.lib` for link) |

Unpack into a directory your app controls (installer, `deps/`, etc.). Do not
mix platforms: the archive matches one OS/CPU.

Python hosts can use `pip install` instead ([python-distribution.md](python-distribution.md)).

## Java — Maven + native path

**Dependency** (version = SDK release):

```kotlin
dependencies {
    implementation("com.offlineprotocol:offline-protocol-java:0.33.0")
}
```

Published when `MAVEN_CENTRAL_PUBLISH` is enabled on the release train
(`offline-protocol-java` jar only).

**Run the JVM** with native access and the library on the path:

```bash
export JAVA_LIBRARY_PATH=/opt/offline-protocol/lib
java --enable-native-access=ALL-UNNAMED -jar your-app.jar
```

Or an absolute override:

```bash
java --enable-native-access=ALL-UNNAMED \
  -Duniffi.component.offline_protocol.libraryOverride=/opt/offline-protocol/lib/liboffline_protocol_uniffi.so \
  -jar your-app.jar
```

JDK **22+** is required (`java.lang.foreign`). Full quick start:
[bindings/java/README.md](../bindings/java/README.md).

`ProtocolConfig` is a long positional record (generated): spell every field or
wrap construction in your own builder (#24 is a separate ergonomics effort).

## C# — .NET + `OFFLINE_PROTOCOL_LIBRARY_DIR`

Add the SDK project (from a release tag or `bindings/csharp`) and point the
loader at the unpacked DLL directory:

```bash
export OFFLINE_PROTOCOL_LIBRARY_DIR=/opt/offline-protocol/lib
dotnet run -c Release
```

On Windows (PowerShell):

```powershell
$env:OFFLINE_PROTOCOL_LIBRARY_DIR = "C:\offline-protocol\lib"
dotnet run -c Release
```

The binding loads `offline_protocol_uniffi.dll` from that directory
([bridges/csharp.md](bridges/csharp.md#cs3-native-library-loading)). Build and
consumer-check details: [bindings/csharp/README.md](../bindings/csharp/README.md).

## Upgrading

Bump the Maven coordinate and replace the native archive from the **same**
`vX.Y.Z` release. Re-run storage conformance if you implement custom providers
([bridges/java.md](bridges/java.md), C# storage tests in the binding).

## Maintainers

Java: `java-library` job → Maven Central (`publish-maven-java`). C#: no registry
publish in CI yet; release assets come from `package-release-assets.sh`. Android
uses `com.offlineprotocol:offline-protocol-sdk` (AAR), not this Java jar.
