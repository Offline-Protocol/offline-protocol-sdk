# Publishing to npm

This guide explains how to publish the React Native bindings package to npm.

## Prerequisites

1. **npm account**: You need an npm account with access to the `@offlineprotocol` organization
2. **Authentication**: Configure npm authentication:
   ```bash
   npm login
   ```
3. **Build TypeScript**: Ensure TypeScript is compiled:
   ```bash
   npm install  # Install dev dependencies
   npm run build  # Compile TypeScript
   ```

## Pre-Publishing Checklist

- [ ] TypeScript is compiled (`lib/` directory exists with `.js` and `.d.ts` files)
- [ ] **Rust FFI libraries are built and included:**
  - [ ] Android: `.so` files in `android/src/main/jniLibs/{arch}/` for all architectures
  - [ ] iOS: `liboffline_protocol_ffi.a` and `offline_protocol.h` in `ios/` directory
- [ ] Version number is updated in `package.json`
- [ ] CHANGELOG is updated (if you maintain one)
- [ ] README.md is up to date
- [ ] All necessary files are included in the `files` field in `package.json`

## Publishing Steps

### 1. Build the Package

```bash
npm install
npm run build
npm run build:rust  # Build Rust FFI libraries for all platforms
```

This:
- Compiles TypeScript to JavaScript in the `lib/` directory
- Builds Rust FFI libraries for Android (all architectures) and iOS
- Places libraries in the correct locations for the npm package

### 2. Verify Package Contents

Check what will be published:

```bash
npm pack --dry-run
```

This shows exactly which files will be included in the package.

### 3. Publish to npm

#### First Time (Initial Publish)

```bash
npm publish --access public
```

The `--access public` flag is required for scoped packages like `@offlineprotocol/react-native`.

#### Subsequent Publishes

```bash
npm version patch  # or minor, major
npm publish
```

This will:
1. Update the version in `package.json`
2. Create a git tag
3. Publish to npm

## Package Structure

The published package will include:

- `lib/` - Compiled JavaScript and TypeScript definitions
- `android/` - Android native module code **including pre-built Rust libraries** (`liboffline_protocol_ffi.so` for all architectures in `android/src/main/jniLibs/`)
- `ios/` - iOS native module code, podspec, **and pre-built Rust library** (`liboffline_protocol_ffi.a` and `offline_protocol.h`)
- `README.md` - Package documentation
- `react-native.config.js` - React Native configuration

**Important**: The Rust FFI libraries ARE included in the npm package. This provides a better user experience - users don't need Rust toolchain to use the package. Make sure to build them before publishing using `npm run build:rust`.

## Post-Publishing

After publishing, verify the package is available:

```bash
npm view @offlineprotocol/react-native
```

## Troubleshooting

### "Package name already exists"

If you get this error, either:
- Use a different version number
- Unpublish the existing version (if within 72 hours): `npm unpublish @offlineprotocol/react-native@version`

### "You do not have permission"

Make sure you're logged in and have access to the `@offlineprotocol` organization on npm.

### Missing files

Check the `files` field in `package.json` and ensure all necessary files are listed.

