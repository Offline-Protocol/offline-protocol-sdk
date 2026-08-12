# Setup Guide

## Prerequisites

- Node.js 20 or higher
- React Native development environment
  - iOS: Xcode and CocoaPods
  - Android: Android Studio and Android SDK
- Rust toolchain (for building native libraries)
- Android NDK (for Android builds)

## First Time Setup

### 1. Build Native Libraries

Navigate to the React Native bindings directory and build native libraries for all platforms.
This also regenerates the UniFFI bindings (Swift, Kotlin and Python are one artifact set off one
UDL), so it needs `uniffi-bindgen` at the crate's pinned version first — it refuses to run on a
mismatch rather than emit bindings whose checksums fail at the app's first call:

```bash
cargo install uniffi --version 0.30.0 --features cli --locked

cd ../../bindings/react-native
npm run build:all
cd ../../examples/react-native-app
```

### 2. Install Dependencies

```bash
npm install
```

### 3. iOS Setup

```bash
cd ios
LANG=en_US.UTF-8 pod install
cd ..
```

### 4. Run the App

**iOS:**
```bash
npm run ios
```

**Android:**
```bash
npm run android
```

## Notes

- The native libraries must be built before running the app for the first time
- For iOS, use the `.xcworkspace` file, not the `.xcodeproj` file
- Physical devices are recommended for testing BLE and Wi-Fi Direct features

