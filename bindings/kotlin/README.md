# The Android library

The sources of the Android library are not in this directory. The library is
built from the bridge sources in `bindings/react-native/android`, which stay
where the React Native module compiles them and where the Rust guards read
them
([ADR 0025](../../docs/adr/0025-native-packages-are-assembled-in-place.md)).
What lives here is the build, and what exists only for the library.

| Path | What it is |
|------|------------|
| `offline-protocol-android/` | The library. Its build file points the source sets at the module's directories and leaves out the three files that need React |
| `consumer-check/` | A separate build: an application that depends on the published library by its coordinates, declares nothing else, and minifies |

Coordinates: `com.offlineprotocol:offline-protocol-sdk`.

## What the library reads, and what it writes again

A value the module declares, and that would drift without anything failing,
is read from the module:

| What | Read from |
|------|-----------|
| Sources and tests | `bindings/react-native/android/src` |
| The manifest | The module's, rewritten at build time without the wake service and its permission |
| `compileSdk`, `minSdk` | The module's `build.gradle` |
| The version of every dependency | The module's `build.gradle` |
| The rules an application's R8 needs | The module's `consumer-rules.pro` |
| The native libraries | `bindings/react-native/android/src/main/jniLibs` |
| The Android Gradle Plugin and Kotlin | The test harness, `bindings/react-native/android-ci-harness/build.gradle` |
| `LICENSE`, `LICENSE-COMMERCIAL.md`, `THIRD-PARTY-NOTICES.md`, `EXPORT.md` | The repository root, packed at the root of the AAR, as the Swift package carries them |

Each is read strictly: from a line that declares it, which has to be there
exactly once. A version in a comment is not a declaration, and two
declarations are a question the build cannot answer, so it stops and says
which.

What is written in both build files is the namespace, the Java level, and
which dependencies there are. The library also declares `androidx.core`,
which the module gets through React Native, and takes JNA as `api`, because
the generated types an application can reach extend JNA's.

The library is built by the toolchain the module's tests run on. A library
compiled by a newer Kotlin than its tests is not the code that was tested,
and cannot be used at all by an application on an older one: Kotlin refuses
metadata more than one version ahead of the compiler reading it.

## Build and test it

With JDK 17, the Android SDK and Gradle 8.9:

```bash
cd bindings/kotlin

# The unit suite, and the library written out as a Maven repository under
# offline-protocol-android/build/maven.
gradle -PVERSION_NAME=0.0.0 \
  :offline-protocol-android:testDebugUnitTest \
  :offline-protocol-android:publishAllPublicationsToStagingRepository

# What is inside the AAR and the sources jar.
PUBLISHED=offline-protocol-android/build/maven/com/offlineprotocol/offline-protocol-sdk/0.0.0
python3 ../../scripts/check_android_aar.py \
  $PUBLISHED/offline-protocol-sdk-0.0.0.aar \
  --sources-jar $PUBLISHED/offline-protocol-sdk-0.0.0-sources.jar

# The library from an application's side: a release build, minified.
gradle -p consumer-check -PVERSION_NAME=0.0.0 :app:assembleRelease
```

Those are the tasks that are supported. `gradle build` and `gradle check`
also run Android lint over the bridge, which reports errors the React Native
build never asked it about.

There is no Gradle wrapper here, as there is none in the React Native
harness: CI installs the version it names.

A pull request builds the library without native libraries, because it does
not cross-compile Rust for Android. With them in
`bindings/react-native/android/src/main/jniLibs`, which is where
`bindings/react-native/scripts/build-uniffi-android.sh` puts them, the same
commands build the library a release would, and `--require-natives` makes
the check insist on all four ABIs. The check also refuses a native library
under any other name, which is what an old build leaves in that directory.

A release builds the library from its own native libraries, runs the same
checks with `--require-natives`, and uploads it to Maven Central when the
`MAVEN_CENTRAL_PUBLISH` variable is on. The build writes an unsigned Maven
repository; the publishing job signs it with gpg
(`scripts/maven-central-bundle.sh`), so the signing key never enters a Gradle
build. See [Cutting a Release](../../CONTRIBUTING.md#the-native-packages-and-pypi).

## What an application has to declare

`android.permission.INTERNET`, for the internet and Nostr transports. The
library's manifest does not declare it, because the module's does not: a
React Native application already has it.

## Adding a source

A new file under `bindings/react-native/android/src/main` is in the library
without being registered anywhere. A file that imports React cannot be: the
library fails to compile, in the pull request that added the import. Put the
React half in `OfflineProtocolModule.kt`, or add the file to `reactOnly` in
the library's build file if all of it needs React.
