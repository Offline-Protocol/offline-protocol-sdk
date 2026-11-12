package consumer

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.util.Log
import com.offlineprotocol.ConfigScenario
import com.offlineprotocol.ConfigScenarios
import com.offlineprotocol.InternetManager
import com.offlineprotocol.OfflineProtocolReferenceStorage
import com.offlineprotocol.StorageConformance
import com.offlineprotocol.TransportManager
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import uniffi.offline_protocol.ProtocolException
import uniffi.offline_protocol.RustBuffer
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

/** Native path: event callback, reference storage, then `start()` (G7). */
fun bootstrapWithReferenceStorage(context: Context, config: ProtocolConfig): OfflineProtocol {
    val protocol = OfflineProtocol(config)
    protocol.setEventCallback(Listener())
    OfflineProtocolReferenceStorage.initializeMls(protocol, context, config.appId, config.profile)
    protocol.start()
    return protocol
}

fun referenceStorageSmokeConfig(): ProtocolConfig =
    ConfigScenarios.forScenario(ConfigScenario.localBle, "consumer-check", "default")

/** Requires the loaded native library (device or emulator), not JVM unit tests alone. */
fun referenceProtocolStateStorageConformanceIsGreen(context: Context): Boolean =
    StorageConformance.verifyReferenceProtocolStateStorage(
        context,
        "consumer-check",
        "default",
    ).isGreen

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
