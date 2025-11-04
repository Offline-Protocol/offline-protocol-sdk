const {getDefaultConfig, mergeConfig} = require('@react-native/metro-config');
const path = require('path');

const defaultConfig = getDefaultConfig(__dirname);

/**
 * Metro configuration
 * https://reactnative.dev/docs/metro
 *
 * This configuration is needed to handle the local @offlineprotocol/react-native package
 */
const config = {
  watchFolders: [
    path.resolve(__dirname, '../..'),  // Watch the entire monorepo
  ],
  resolver: {
    // Make sure Metro can find modules in the local package
    nodeModulesPaths: [
      path.resolve(__dirname, 'node_modules'),
      path.resolve(__dirname, '../../bindings/react-native/node_modules'),
    ],
    extraNodeModules: {
      '@offlineprotocol/react-native': path.resolve(__dirname, '../../bindings/react-native'),
    },
  },
};

module.exports = mergeConfig(defaultConfig, config);
