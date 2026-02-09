module.exports = function (api) {
  // Disable Babel cache to ensure .env file changes are detected
  api.cache(false);

  return {
    presets: ['module:@react-native/babel-preset'],
    plugins: [
      [
        'module:react-native-dotenv',
        {
          moduleName: '@env',
          path: '.env',
          safe: false,
          allowUndefined: true,
          blocklist: null,
          allowlist: null,
          verbose: false,
        },
      ],
    ],
  };
};
