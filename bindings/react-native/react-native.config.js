module.exports = {
  dependency: {
    platforms: {
      android: {
        sourceDir: './android',
        packageImportPath: 'import com.offlineprotocol.OfflineProtocolPackage;',
      },
      ios: {
        podspecPath: './ios/OfflineProtocol.podspec',
      },
    },
  },
};
