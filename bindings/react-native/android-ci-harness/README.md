# Android CI harness

A minimal standalone Gradle project whose only purpose is to run the Android
library module's **JVM unit tests** (`../android/src/test`) in CI and locally,
without a full React Native app.

## Why this exists

`../android` is a React Native library module. It uses legacy
`apply plugin: 'com.android.library'` and has no `settings.gradle` or Gradle
wrapper of its own, because it is meant to be configured by a *host app's*
build. That makes it impossible to run `./gradlew test` against it directly.

This harness:

- supplies the Android Gradle Plugin + Kotlin plugin on the buildscript
  classpath (`build.gradle`),
- includes `../android` as the `:offlineprotocol` subproject **without editing
  that module's `build.gradle`**, so real RN-app consumption is unaffected.

In CI there is no `node_modules`, so the module's `reactNativeExists` check is
false and it takes its built-in "standalone build or testing" path
(`compileOnly` react-android) — the path this harness exercises.

## Run it

```bash
# Gradle 8.7+ and JDK 17 required (Android SDK auto-provisioned by AGP).
cd bindings/react-native/android-ci-harness
gradle :offlineprotocol:testDebugUnitTest
```

The same task runs in the `Android Unit Tests` job in `.github/workflows/ci.yml`.

## Version notes

Kotlin is pinned to **1.9.24** to match the module's current source. Moving to
Kotlin 2.x requires the `optString`/nullability fixes tracked in `todo.md`
first, or the module will not compile under strict 2.x platform-type rules.
AGP 8.5.2 / Gradle 8.9 / `compileSdk 34` / JDK 17 form the validated matrix.
