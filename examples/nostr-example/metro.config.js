const {getDefaultConfig, mergeConfig} = require('@react-native/metro-config');
const path = require('path');

const defaultConfig = getDefaultConfig(__dirname);

function exclusionList(additionalExclusions) {
  const defaultBlockList =
    defaultConfig.resolver.blockList ||
    defaultConfig.resolver.blacklistRE ||
    new RegExp('^$');
  return new RegExp(
    '(' +
      (additionalExclusions || [])
        .map(function (regexp) {
          return regexp.source;
        })
        .join('|') +
      '|' +
      defaultBlockList.source +
      ')',
  );
}

const bindingsPath = path.resolve(__dirname, '../../bindings/react-native');

const config = {
  watchFolders: [
    path.resolve(__dirname, '../..'), // Watch the entire monorepo
  ],
  resolver: {
    nodeModulesPaths: [
      path.resolve(__dirname, 'node_modules'),
      path.resolve(__dirname, '../../bindings/react-native/node_modules'),
    ],
    blockList: exclusionList([
      new RegExp(
        `${path.resolve(bindingsPath, 'node_modules/react-native')}/.*`,
      ),
      new RegExp(
        `${path.resolve(bindingsPath, 'node_modules/react')}/.*`,
      ),
    ]),
    extraNodeModules: {
      '@offline-protocol/mesh-sdk': bindingsPath,
      'react-native': path.resolve(__dirname, 'node_modules/react-native'),
      react: path.resolve(__dirname, 'node_modules/react'),
    },
  },
};

module.exports = mergeConfig(defaultConfig, config);
