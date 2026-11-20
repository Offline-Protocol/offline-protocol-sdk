package consumer

// Canonical native Android imports. The engine is uniffi.offline_protocol.*;
// the hand-written bridge types are com.offlineprotocol.*. OfflineProtocolModule
// is React Native only and is not in the library. See
// docs/android-jni-and-imports.md.

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.util.Log
import com.offlineprotocol.InternetManager
import com.offlineprotocol.OfflineProtocolDevelopmentCleartextConfig
import com.offlineprotocol.OfflineProtocolJniLibrary
import com.offlineprotocol.OfflineProtocolReferenceStorage
import com.offlineprotocol.StorageConformance
import com.offlineprotocol.TransportManager
import java.util.Collections
import java.util.IdentityHashMap
import uniffi.offline_protocol.AppState
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import uniffi.offline_protocol.ProtocolException
import uniffi.offline_protocol.ProtocolStateStorageProvider
import uniffi.offline_protocol.RustBuffer
import uniffi.offline_protocol.SendMessageOptions
import uniffi.offline_protocol.TelemetryConfig
import uniffi.offline_protocol.TelemetryOs
import uniffi.offline_protocol.appId
import uniffi.offline_protocol.deriveAddress

// What an application writes first: build the engine, listen to it, start it,
// hand it to a transport. Never run. It has to compile, and the application
// around it has to survive R8.

class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Log.i("ConsumerCheck", address(null, listOf()) ?: "no address")
    }
}

class Listener : EventCallback {
    override fun onEvent(eventJson: String) {}
}

fun start(config: ProtocolConfig): OfflineProtocol {
    val protocol = OfflineProtocol(config)
    protocol.setEventCallback(Listener())
    protocol.start()
    return protocol
}

// A native application's start: listen, open the built-in stores, then start.
// MLS must be initialized before start(), and the adoption warning is the only
// signal that this account lost an earlier identity.
fun startWithReferenceStorage(context: Context, config: ProtocolConfig): OfflineProtocol {
    val protocol = OfflineProtocol(config)
    protocol.setEventCallback(Listener())
    val adoption = OfflineProtocolReferenceStorage.initializeMls(protocol, context, config.appLabel, config.profile)
    adoption.warning?.let { Log.e("ConsumerCheck", it) }
    protocol.start()
    return protocol
}

// Sign out for good: stop the engine, release it, then wipe. Releasing the
// engine flushes its state to storage, so an engine released after the wipe
// writes that state back into the account the wipe just emptied. Every other
// holder (a transport manager, a service) must have let go first.
fun signOut(context: Context, protocol: OfflineProtocol, config: ProtocolConfig) {
    protocol.stop()
    protocol.close()
    OfflineProtocolReferenceStorage.wipe(context, config.appLabel, config.profile)
}

// Verify a protocol-state backend before wiring it in (C11): green means
// supported. Run it off the main thread, and on a scratch instance: the suite
// blocks on writes of several megabytes.
// Code written before 0.35 named the app label `appId`. The deprecated
// aliases keep it compiling.
@Suppress("DEPRECATION")
fun labelUnderItsOldName(config: ProtocolConfig): String {
    val options = SendMessageOptions()
    options.appId = config.appId
    return options.appId ?: config.appId
}

// Lab-only config factory (compile-checked; do not use in production apps).
fun labCleartextConfig(appLabel: String, profile: String): ProtocolConfig =
    OfflineProtocolDevelopmentCleartextConfig.make(appLabel, profile)

fun isSupported(backend: ProtocolStateStorageProvider): Boolean {
    val report = StorageConformance.run(backend)
    report.failures.forEach { Log.e("ConsumerCheck", "${it.check}: ${it.detail}") }
    return report.isGreen
}

fun address(protocol: OfflineProtocol?, publicKey: List<UByte>): String? =
    try {
        protocol?.localAddress() ?: deriveAddress(publicKey)
    } catch (error: ProtocolException) {
        null
    }

fun relay(context: Context, protocol: OfflineProtocol): TransportManager =
    InternetManager(context, protocol, deviceId = "device", appId = "app")

// The generated types an application does not call and can still reach. They
// extend JNA's, so this compiles only while the library's metadata hands JNA
// to whoever compiles against it.
fun reachesJna(): Long = RustBuffer.ByValue().len

/** Compile-checked JNI names for manual integrators (Maven AAR users can ignore). */
fun jniNamesForDocs(): Pair<String, String> =
    OfflineProtocolJniLibrary.LOADED_BASENAME to OfflineProtocolJniLibrary.CARGO_OUTPUT_BASENAME

// Telemetry (C12). On the native path the application is the binding: it
// fills the platform fields and owns the lifecycle. Never run. This mirrors
// installProcessLifecycleWatcher in the React Native module
// (bindings/react-native/android/.../OfflineProtocolModule.kt), which is not
// in the library.

/** The platform fields come from the device, never from app configuration. */
fun telemetryConfig(apiKey: String, appId: String, appVersion: String?): TelemetryConfig =
    TelemetryConfig(
        apiKey = apiKey,
        appId = appId,
        os = TelemetryOs.ANDROID,
        // The API level, as the other Android bindings report it.
        osMajor = Build.VERSION.SDK_INT.toUShort(),
        appVersion = appVersion,
        debug = null,
        flushIntervalMs = null,
        maxBatchBytes = null,
        maxBufferedRecords = null,
        includeDeviceId = null,
        scrubIds = null,
        mlsVerbosity = null,
        metricsCadenceMs = null,
        routingDiagnostic = null,
        mlsSamplingBypass = null,
    )

/**
 * Draws the telemetry session boundary when the process leaves the
 * foreground: when its last started activity stops. Never on
 * `Activity.onPause`, which fires for a permission dialog, the share sheet or
 * any translucent activity, and would count one iOS session as several on
 * Android.
 *
 * Register it no later than the first activity's `onCreate`
 * ([ConsumerApplication] shows where), so no `onStart` is missed: a missed one
 * leaves the set empty while an activity is on screen, and [appState] then
 * seeds the pipe on the wrong side of the boundary.
 */
class TelemetryLifecycleForwarder : Application.ActivityLifecycleCallbacks {
    /** Set at enable, cleared before the engine is released. Read on main. */
    @Volatile
    var protocol: OfflineProtocol? = null

    // By identity rather than counted: with two activities the sequence is
    // start B, stop A, and a count that reached zero there would background
    // an app that is fully on screen. Written on main, read by [appState]
    // from whichever thread enables telemetry, so synchronized.
    private val started: MutableSet<Activity> =
        Collections.synchronizedSet(Collections.newSetFromMap(IdentityHashMap<Activity, Boolean>()))

    /** The state to seed `enableTelemetry` with. */
    val appState: AppState
        get() = if (started.isEmpty()) AppState.BACKGROUND else AppState.ACTIVE

    override fun onActivityStarted(activity: Activity) {
        started.add(activity)
        // An ACTIVE with no BACKGROUND before it is not an edge; the pipe ignores it.
        notifyQuietly(AppState.ACTIVE)
    }

    override fun onActivityStopped(activity: Activity) {
        started.remove(activity)
        // A rotation stops and restarts the activity without leaving the foreground.
        if (started.isEmpty() && !activity.isChangingConfigurations) {
            // The pipe emits its summary and wakes the uploader; nothing here blocks.
            notifyQuietly(AppState.BACKGROUND)
        }
    }

    override fun onActivityDestroyed(activity: Activity) {
        // An activity destroyed without a stop would hold the background edge off.
        started.remove(activity)
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
    override fun onActivityResumed(activity: Activity) {}
    override fun onActivityPaused(activity: Activity) {}
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

    // Every exception: one escaping a lifecycle callback takes the process
    // down, and a handle closed by a sign-out on another thread throws here.
    // Losing the edge is right then: the pipe flushed and stopped on teardown.
    private fun notifyQuietly(state: AppState) {
        try {
            protocol?.notifyAppState(state)
        } catch (e: Exception) {
            Log.d("ConsumerCheck", "telemetry lifecycle edge skipped", e)
        }
    }
}

/** Where the watcher is registered: before any activity can start. */
class ConsumerApplication : Application() {
    val telemetryLifecycle = TelemetryLifecycleForwarder()

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(telemetryLifecycle)
    }
}

// Hand the watcher the engine, then enable seeded from it. Before signOut
// releases the engine, set `lifecycle.protocol = null`.
fun enableTelemetry(
    lifecycle: TelemetryLifecycleForwarder,
    protocol: OfflineProtocol,
    apiKey: String,
    appId: String,
    appVersion: String?,
) {
    lifecycle.protocol = protocol
    protocol.enableTelemetry(telemetryConfig(apiKey, appId, appVersion), lifecycle.appState)
}
