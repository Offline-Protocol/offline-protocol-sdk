// `appId`, the name the app label had before 0.35, on the three generated
// records that carried it. The generated code takes only `appLabel`, so code
// that read or set `config.appId` would otherwise stop compiling on upgrade.
// Only the name changed: the value still selects the storage namespace and
// is stamped on every frame, so a shipped app keeps passing the same string.
//
// Declared in the generated package, so `import uniffi.offline_protocol.*`
// brings these in with the records; an explicit import names them
// (`import uniffi.offline_protocol.appId`). The generated constructors'
// named argument `appId =` cannot be aliased this way: write `appLabel =`
// (docs/UPGRADING.md §33).
@file:JvmName("AppLabelAliases")

package uniffi.offline_protocol

private const val APP_ID_DEPRECATION =
    "appId on the mesh is now appLabel. Your App ID (app_…) from the dashboard is what turns on telemetry"

/** The old name of [ProtocolConfig.appLabel]. */
@Deprecated(APP_ID_DEPRECATION, ReplaceWith("appLabel"))
var ProtocolConfig.appId: String
    get() = appLabel
    set(value) {
        appLabel = value
    }

/** The old name of [SendMessageOptions.appLabel]. */
@Deprecated(APP_ID_DEPRECATION, ReplaceWith("appLabel"))
var SendMessageOptions.appId: String?
    get() = appLabel
    set(value) {
        appLabel = value
    }

/** The old name of [MediaSendOptions.appLabel]. */
@Deprecated(APP_ID_DEPRECATION, ReplaceWith("appLabel"))
var MediaSendOptions.appId: String?
    get() = appLabel
    set(value) {
        appLabel = value
    }
