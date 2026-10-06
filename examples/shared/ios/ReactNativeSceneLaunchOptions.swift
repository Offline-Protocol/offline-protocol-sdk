import UIKit

/// Maps UIScene cold-start context into `UIApplication.LaunchOptionsKey` values for React Native.
enum ReactNativeSceneLaunchOptions {
  static func merging(
    _ launchOptions: [UIApplication.LaunchOptionsKey: Any]?,
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
