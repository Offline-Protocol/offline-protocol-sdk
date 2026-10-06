import UIKit
import React
import React_RCTAppDelegate
import ReactAppDependencyProvider

@main
class AppDelegate: UIResponder, UIApplicationDelegate {
  var window: UIWindow?
  var reactNativeDelegate: ReactNativeDelegate?
  var reactNativeFactory: RCTReactNativeFactory?
  private(set) var launchOptions: [UIApplication.LaunchOptionsKey: Any]?

  func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    let delegate = ReactNativeDelegate()
    let factory = RCTReactNativeFactory(delegate: delegate)
    delegate.dependencyProvider = RCTAppDependencyProvider()

    reactNativeDelegate = delegate
    reactNativeFactory = factory
    self.launchOptions = launchOptions

    return true
  }

  func application(
    _ application: UIApplication,
    configurationForConnecting connectingSceneSession: UISceneSession,
    options: UIScene.ConnectionOptions
  ) -> UISceneConfiguration {
    let configuration = UISceneConfiguration(
      name: "Default Configuration",
      sessionRole: connectingSceneSession.role
    )
    configuration.delegateClass = SceneDelegate.self
    return configuration
  }

  func reactNativeLaunchOptions(
    sceneConnectionOptions: UIScene.ConnectionOptions
  ) -> [UIApplication.LaunchOptionsKey: Any]? {
    var options = launchOptions ?? [:]

    if let url = sceneConnectionOptions.urlContexts.first?.url {
      options[.url] = url
    }

    if let response = sceneConnectionOptions.notificationResponse {
      options[.remoteNotification] = response.notification.request.content.userInfo
    }

    if let userActivity = sceneConnectionOptions.userActivities.first {
      options[.userActivityType] = userActivity.activityType
      options[.userActivityDictionary] = [
        "UIApplicationLaunchOptionsUserActivityKey": userActivity,
      ]
    }

    return options.isEmpty ? nil : options
  }
}

class ReactNativeDelegate: RCTDefaultReactNativeFactoryDelegate {
  override func sourceURL(for bridge: RCTBridge) -> URL? {
    self.bundleURL()
  }

  override func bundleURL() -> URL? {
#if DEBUG
#if targetEnvironment(simulator)
    return RCTBundleURLProvider.sharedSettings().jsBundleURL(forBundleRoot: "index")
#else
    if let embedded = Bundle.main.url(forResource: "main", withExtension: "jsbundle") {
      return embedded
    }
    return RCTBundleURLProvider.sharedSettings().jsBundleURL(forBundleRoot: "index")
#endif
#else
    return Bundle.main.url(forResource: "main", withExtension: "jsbundle")
#endif
  }
}
