# npm Publishing Checklist

## Pre-Publishing Steps

### 1. Build TypeScript and Rust Libraries
```bash
npm install
npm run build        # Build TypeScript
npm run build:rust   # Build Rust FFI libraries for all platforms
```
This creates:
- `lib/` directory with compiled JavaScript and TypeScript definitions
- Rust libraries in `android/src/main/jniLibs/` and `ios/` directories

### 2. Verify Package Contents
```bash
npm pack --dry-run
```
This shows exactly what will be included in the published package.

Expected files:
- `lib/` - Compiled JavaScript and TypeScript definitions
- `android/` - Android native module (Kotlin, JNI, build files) **including Rust libraries** in `android/src/main/jniLibs/{arch}/`
- `ios/` - iOS native module (Swift, Objective-C, podspec) **including Rust library** (`liboffline_protocol_ffi.a` and `offline_protocol.h`)
- `README.md` - Documentation
- `react-native.config.js` - React Native configuration
- `package.json` - Package metadata

### 3. Check Version
Ensure the version in `package.json` is correct:
- `0.1.0` for initial release
- Use semantic versioning (major.minor.patch)

### 4. Verify Repository Access
Make sure you have access to publish to `@offlineprotocol` organization:
```bash
npm whoami
npm access ls-packages @offlineprotocol
```

## Publishing Commands

### First Time (Initial Publish)
```bash
npm login
npm publish --access public
```

### Update Version and Publish
```bash
npm version patch  # or minor, major
npm publish
```

### Verify Publication
```bash
npm view @offlineprotocol/react-native
```

## Important Notes

1. **Rust Libraries Must Be Built**: The npm package INCLUDES the compiled Rust FFI libraries. You must build them before publishing:
   - Run `npm run build:rust` to build for all platforms
   - The `prepublishOnly` script automatically runs `npm run build && npm run build:rust`
   - Libraries must be in: `android/src/main/jniLibs/{arch}/liboffline_protocol_ffi.so` and `ios/liboffline_protocol_ffi.a`

2. **TypeScript Must Be Built**: The `lib/` directory must exist before publishing. The `prepublishOnly` script automatically runs `npm run build`.

3. **Scoped Package**: The `--access public` flag is required for scoped packages like `@offlineprotocol/react-native`.

4. **Files Field**: Only files listed in the `files` field in `package.json` will be published. Source TypeScript files (`src/`) are excluded, but Rust libraries in `android/` and `ios/` are included.

5. **Package Size**: Including pre-built Rust libraries increases package size but provides a much better user experience (no Rust toolchain required).

## Troubleshooting

- **"Package name already exists"**: Increment the version number
- **"You do not have permission"**: Check npm organization access
- **Missing files**: Verify the `files` field in `package.json`

