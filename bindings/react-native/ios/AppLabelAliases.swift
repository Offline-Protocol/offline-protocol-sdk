import Foundation

// `appId`, the name the app label had before 0.35, on the three generated
// records that carried it. The generated code takes only `appLabel`, so code
// that read or set `config.appId` would otherwise stop compiling on upgrade.
// Only the name changed: the value still selects the storage namespace and
// is stamped on every frame, so a shipped app keeps passing the same string.
//
// The generated memberwise initializers cannot be aliased this way: write
// `ProtocolConfig(appLabel: …)` (docs/UPGRADING.md §33).

extension ProtocolConfig {
    /// The old name of `appLabel`.
    @available(*, deprecated, message: "appId on the mesh is now appLabel. Your App ID (app_…) from the dashboard is what turns on telemetry")
    public var appId: String {
        get { appLabel }
        set { appLabel = newValue }
    }
}

extension SendMessageOptions {
    /// The old name of `appLabel`.
    @available(*, deprecated, message: "appId on the mesh is now appLabel. Your App ID (app_…) from the dashboard is what turns on telemetry")
    public var appId: String? {
        get { appLabel }
        set { appLabel = newValue }
    }
}

extension MediaSendOptions {
    /// The old name of `appLabel`.
    @available(*, deprecated, message: "appId on the mesh is now appLabel. Your App ID (app_…) from the dashboard is what turns on telemetry")
    public var appId: String? {
        get { appLabel }
        set { appLabel = newValue }
    }
}
