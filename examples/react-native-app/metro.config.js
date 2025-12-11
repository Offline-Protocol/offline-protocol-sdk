const {getDefaultConfig, mergeConfig} = require('@react-native/metro-config');
const path = require('path');

const defaultConfig = getDefaultConfig(__dirname);

function exclusionList(additionalExclusions) {
  const defaultBlockList = defaultConfig.resolver.blockList || defaultConfig.resolver.blacklistRE || new RegExp('^$');
  return new RegExp(
    "(" +
      (additionalExclusions || []).map(function(regexp) {
        return regexp.source;
      }).join("|") +
      "|" +
      defaultBlockList.source +
      ")"
  );
}

/**
 * Metro configuration
 * https://reactnative.dev/docs/metro
 *
 * This configuration is needed to handle the local @offline-protocol/mesh-sdk package
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
    blockList: exclusionList([
      // Exclude react-native and react from the bindings to avoid duplicates
      new RegExp(
        `${path.resolve(__dirname, '../../bindings/react-native/node_modules/react-native')}/.*`,
      ),
      new RegExp(
        `${path.resolve(__dirname, '../../bindings/react-native/node_modules/react')}/.*`,
      ),
    ]),
    extraNodeModules: {
      '@offline-protocol/mesh-sdk': path.resolve(__dirname, '../../bindings/react-native'),
      'react-native': path.resolve(__dirname, 'node_modules/react-native'),
      'react': path.resolve(__dirname, 'node_modules/react'),
    },
  },
};

module.exports = mergeConfig(defaultConfig, config);
