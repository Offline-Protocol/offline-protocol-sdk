# Troubleshooting Guide

This guide helps resolve common issues when running the example app.

## Current Issues Detected

### ❌ Android: Java Runtime Not Found
**Error**: "Unable to locate a Java Runtime"

**Solution**: Install Java JDK 17 (required for React Native 0.82+)

```bash
# Install using Homebrew
brew install openjdk@17

# Add to your shell profile (~/.zshrc or ~/.bash_profile)
export JAVA_HOME=$(/usr/libexec/java_home -v 17)
export PATH="$JAVA_HOME/bin:$PATH"

# Reload your shell
source ~/.zshrc  # or source ~/.bash_profile
```

After installation, verify:
```bash
java -version
# Should show: openjdk version "17.x.x"
```

### ❌ Android: ADB Command Not Found
**Error**: "/bin/sh: adb: command not found"

**Solution**: Add Android SDK to PATH

```bash
# Add to your shell profile (~/.zshrc or ~/.bash_profile)
export ANDROID_HOME=$HOME/Library/Android/sdk
export PATH=$PATH:$ANDROID_HOME/emulator
export PATH=$PATH:$ANDROID_HOME/platform-tools
export PATH=$PATH:$ANDROID_HOME/tools
export PATH=$PATH:$ANDROID_HOME/tools/bin

# Reload your shell
source ~/.zshrc
```

Verify:
```bash
adb version
# Should show Android Debug Bridge version
```

### ⚠️ iOS: Build Failed (Error Code 70)
**Error**: "xcodebuild" exited with error code '70'

**Possible causes and solutions:**

#### 1. Pods Not Installed
```bash
cd ios
rm -rf Pods Podfile.lock
LANG=en_US.UTF-8 pod install
cd ..
```

#### 2. Native Library Missing
Ensure iOS library is built:
```bash
cd ../../bindings/react-native
npm run build:ios
cd ../../examples/react-native-app
cd ios && pod install && cd ..
```

#### 3. Code Signing Issue
Open in Xcode to configure signing:
```bash
open ios/OfflineProtocolExample.xcworkspace
```

Then in Xcode:
- Select "OfflineProtocolExample" project
- Select "OfflineProtocolExample" target
- Go to "Signing & Capabilities"
- Select your team or use automatic signing

#### 4. Clean Build
```bash
cd ios
xcodebuild clean -workspace OfflineProtocolExample.xcworkspace -scheme OfflineProtocolExample
cd ..
```

## Complete Setup Guide

### iOS Setup

1. **Xcode** (✅ Already installed - Xcode 16.4)
   ```bash
   xcode-select --install  # If needed
   ```

2. **CocoaPods**
   ```bash
   sudo gem install cocoapods
   ```

3. **Build Native Library**
   ```bash
   cd ../../bindings/react-native
   npm run build:ios
   cd ../../examples/react-native-app
   ```

4. **Install Pods**
   ```bash
   cd ios
   LANG=en_US.UTF-8 pod install
   cd ..
   ```

5. **Run**
   ```bash
   npm run ios
   ```

### Android Setup

1. **Java JDK 17** (❌ Not installed)
   ```bash
   brew install openjdk@17
   ```

2. **Android Studio**
   - Download from https://developer.android.com/studio
   - During installation, ensure these are selected:
     - Android SDK
     - Android SDK Platform
     - Android Virtual Device (AVD)

3. **Environment Variables**
   Add to `~/.zshrc`:
   ```bash
   export JAVA_HOME=$(/usr/libexec/java_home -v 17)
   export ANDROID_HOME=$HOME/Library/Android/sdk
   export PATH=$PATH:$ANDROID_HOME/emulator
   export PATH=$PATH:$ANDROID_HOME/platform-tools
   export PATH=$PATH:$ANDROID_HOME/tools
   export PATH=$PATH:$ANDROID_HOME/tools/bin
   ```

4. **Build Native Library**
   ```bash
   cd ../../bindings/react-native
   npm run build:android
   cd ../../examples/react-native-app
   ```

5. **Create/Start Emulator**
   ```bash
   # List available AVDs
   emulator -list-avds
   
   # If none exist, create one via Android Studio:
   # Tools > Device Manager > Create Device
   
   # Start an emulator
   emulator -avd <device-name> &
   ```

6. **Run**
   ```bash
   npm run android
   ```

## Quick Fixes

### Reset Everything

**iOS:**
```bash
cd ios
rm -rf Pods Podfile.lock
rm -rf ~/Library/Developer/Xcode/DerivedData/*
LANG=en_US.UTF-8 pod install
cd ..
npm run ios
```

**Android:**
```bash
cd android
./gradlew clean
cd ..
rm -rf node_modules
npm install
npm run android
```

### React Native Doctor

Run the diagnostic tool:
```bash
npx react-native doctor
```

This will check your environment and suggest fixes.

## Common Issues

### "Command PhaseScriptExecution failed" (iOS)
- Clean build folder in Xcode: Product > Clean Build Folder
- Delete derived data: `rm -rf ~/Library/Developer/Xcode/DerivedData`
- Reinstall pods

### "SDK location not found" (Android)
Create `android/local.properties`:
```
sdk.dir=/Users/YOUR_USERNAME/Library/Android/sdk
```

### Metro bundler port in use
```bash
killall node
npm start -- --reset-cache
```

### Pods installation fails
```bash
sudo gem install ffi
cd ios
pod install --repo-update
```

## Environment Check Script

Save as `check-env.sh`:
```bash
#!/bin/bash
echo "=== React Native Environment Check ==="
echo ""

echo "Node.js:"
node -v || echo "❌ Not installed"

echo "npm:"
npm -v || echo "❌ Not installed"

echo "Watchman:"
watchman --version || echo "⚠️  Not installed (optional)"

echo "Java:"
java -version 2>&1 | head -1 || echo "❌ Not installed"

echo "Android SDK:"
if [ -d "$ANDROID_HOME" ]; then
  echo "✅ Found at $ANDROID_HOME"
else
  echo "❌ ANDROID_HOME not set"
fi

echo "ADB:"
adb version 2>&1 | head -1 || echo "❌ Not found"

echo "Xcode:"
xcodebuild -version 2>&1 | head -1 || echo "❌ Not installed"

echo "CocoaPods:"
pod --version || echo "❌ Not installed"

echo "iOS Targets:"
rustup target list | grep -E "(aarch64-apple-ios|x86_64-apple-ios)" | grep installed || echo "⚠️  Not all installed"

echo "Android Targets:"
rustup target list | grep -E "android" | grep installed || echo "⚠️  Not all installed"

echo ""
echo "=== Native Libraries ==="
echo "iOS library:"
ls -lh ../../bindings/react-native/ios/libs/liboffline_protocol_ffi.a 2>/dev/null || echo "❌ Not built"

echo "Android libraries:"
ls -d ../../bindings/react-native/android/src/main/jniLibs/*/ 2>/dev/null || echo "❌ Not built"
```

Run it:
```bash
chmod +x check-env.sh
./check-env.sh
```

## Priority Actions

To get the app running quickly:

### For iOS (Recommended - Easier)

1. Build native library:
   ```bash
   cd ../../bindings/react-native
   npm run build:ios
   cd ../../examples/react-native-app
   ```

2. Install pods:
   ```bash
   cd ios
   LANG=en_US.UTF-8 pod install
   cd ..
   ```

3. Open in Xcode to fix signing:
   ```bash
   open ios/OfflineProtocolExample.xcworkspace
   ```
   - Configure signing in Xcode
   - Hit Run (Cmd+R)

### For Android (Requires More Setup)

1. Install Java 17:
   ```bash
   brew install openjdk@17
   ```

2. Add to `~/.zshrc` and reload:
   ```bash
   export JAVA_HOME=$(/usr/libexec/java_home -v 17)
   export ANDROID_HOME=$HOME/Library/Android/sdk
   export PATH=$PATH:$ANDROID_HOME/platform-tools
   source ~/.zshrc
   ```

3. Create emulator in Android Studio

4. Build and run:
   ```bash
   cd ../../bindings/react-native
   npm run build:android
   cd ../../examples/react-native-app
   npm run android
   ```

## Getting Help

If issues persist:

1. **Run React Native doctor:**
   ```bash
   npx react-native doctor
   ```

2. **Check detailed build logs:**
   - iOS: Open `.xcworkspace` in Xcode
   - Android: Run `cd android && ./gradlew app:installDebug --stacktrace`

3. **Common resources:**
   - [React Native Environment Setup](https://reactnative.dev/docs/environment-setup)
   - [React Native Troubleshooting](https://reactnative.dev/docs/troubleshooting)
   - [iOS Setup Issues](https://reactnative.dev/docs/running-on-device)

## Next Steps

Once environment is set up:

1. ✅ iOS native library built
2. ✅ Android native libraries built
3. ⬜ Java 17 installed
4. ⬜ Android environment configured
5. ⬜ iOS signing configured
6. ⬜ App running successfully

After setup, the app should run smoothly for development!

