# Build and Test Instructions

This document provides instructions for building and testing the example app.

## Build Status

### ✅ Completed
- [x] React Native app initialized
- [x] iOS and Android configurations complete
- [x] All UI components implemented
- [x] All SDK features integrated
- [x] TypeScript compilation passing
- [x] No linter errors
- [x] Documentation complete

### 📋 Pre-requisites for Running

Before running the app, ensure the native libraries are built:

#### Build iOS Libraries

```bash
cd ../../bindings/react-native
npm run build:ios
```

This builds the Rust library for iOS. Note: Currently builds for device architecture (arm64). The library works on both physical devices and Apple Silicon simulators.

**For Intel Mac simulators**: You may need to build specifically for x86_64:
```bash
cd ../..
cargo build --release --target x86_64-apple-ios --package offline-protocol-ffi
cp target/x86_64-apple-ios/release/liboffline_protocol_ffi.a bindings/react-native/ios/libs/
```

#### Build Android Libraries

```bash
cd ../../bindings/react-native
npm run build:android
```

This builds the Rust library for all Android architectures (arm64-v8a, armeabi-v7a, x86, x86_64).

## Testing Checklist

### Code Quality ✅

- [x] TypeScript compilation: `npx tsc --noEmit` - **PASSED**
- [x] No linter errors
- [x] All components properly typed
- [x] Event handling implemented correctly
- [x] Error boundaries in place

### iOS Testing

To test on iOS:

1. **Build native libraries:**
   ```bash
   cd bindings/react-native
   npm run build:ios
   ```

2. **Install pods:**
   ```bash
   cd examples/react-native-app/ios
   LANG=en_US.UTF-8 pod install
   cd ..
   ```

3. **Run the app:**
   ```bash
   npm run ios
   ```

4. **Test scenarios:**
   - [ ] Protocol starts successfully
   - [ ] Message sending works
   - [ ] Events are received and displayed
   - [ ] UI is responsive
   - [ ] No crashes or errors

### Android Testing

To test on Android:

1. **Build native libraries:**
   ```bash
   cd bindings/react-native
   npm run build:android
   ```

2. **Run the app:**
   ```bash
   cd examples/react-native-app
   npm run android
   ```

3. **Test scenarios:**
   - [ ] Protocol starts successfully
   - [ ] Message sending works
   - [ ] Events are received and displayed
   - [ ] UI is responsive
   - [ ] Permissions are requested correctly
   - [ ] No crashes or errors

### Multi-Device Testing

For full offline functionality testing:

1. **Install on 2+ devices**
2. **Note each device's User ID**
3. **Start protocol on all devices**
4. **Test scenarios:**
   - [ ] Send message between devices
   - [ ] Verify message delivery
   - [ ] Check transport switching
   - [ ] Observe neighbor discovery
   - [ ] Test relay promotion (3+ devices)
   - [ ] Monitor network metrics

## Validation Summary

### Static Analysis ✅

All static checks have passed:
- TypeScript types are correct
- No compilation errors
- No linter warnings
- Code follows React Native best practices

### Runtime Testing 🔄

Runtime testing requires:
- Native library builds for all architectures
- Physical devices or simulators
- Appropriate permissions granted
- Network connectivity for testing

**Next Steps for Deployment:**
1. Build native libraries using the scripts in `bindings/react-native/scripts/`
2. Run on iOS simulator/device
3. Run on Android emulator/device
4. Perform manual testing of all features
5. Test on multiple devices for offline features

## Known Issues

None. The app is ready for testing once native libraries are built.

## Support

For build issues:
- Check [README.md](./README.md) troubleshooting section
- Review [INTEGRATION_GUIDE.md](./INTEGRATION_GUIDE.md)
- Ensure all prerequisites are installed
- Verify Rust toolchain is up to date

## Automated Testing

To add automated tests:

1. **Unit tests for hooks:**
   ```typescript
   // src/hooks/__tests__/useOfflineProtocol.test.ts
   ```

2. **Component tests:**
   ```typescript
   // src/components/__tests__/EventLog.test.tsx
   ```

3. **E2E tests:**
   - Consider using Detox or Appium
   - Test full user flows
   - Verify native module integration

## Continuous Integration

For CI/CD:

1. Add GitHub Actions workflow
2. Run TypeScript checks
3. Run linter
4. Build for iOS and Android
5. Run automated tests

Example workflow:
```yaml
name: Test Example App
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions/setup-node@v3
      - run: cd examples/react-native-app && npm install
      - run: cd examples/react-native-app && npx tsc --noEmit
      - run: cd examples/react-native-app && npm run lint
```

