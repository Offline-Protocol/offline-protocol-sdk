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

Navigate to the React Native bindings directory and build native libraries for all platforms:

```bash
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

