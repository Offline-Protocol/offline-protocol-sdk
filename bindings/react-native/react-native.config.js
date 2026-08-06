module.exports = {
  dependency: {
    platforms: {
      // Both platforms autolink. iOS resolves MeshSdk.podspec at the package
      // root — React Native's autolinking globs "*.podspec" there and does not
      // recurse, so the podspec must not be moved into ios/.
      ios: {},
      android: {},
    },
  },
};

