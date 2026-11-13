package consumer

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.util.Log
import com.offlineprotocol.InternetManager
import com.offlineprotocol.OfflineProtocolReferenceStorage
import com.offlineprotocol.StorageConformance
import com.offlineprotocol.TransportManager
import uniffi.offline_protocol.EventCallback
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.ProtocolConfig
import uniffi.offline_protocol.ProtocolException
import uniffi.offline_protocol.ProtocolStateStorageProvider
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

// A native application's start: listen, open the built-in stores, then start.
// MLS must be initialized before start(), and the adoption warning is the only
// signal that this account lost an earlier identity.
fun startWithReferenceStorage(context: Context, config: ProtocolConfig): OfflineProtocol {
    val protocol = OfflineProtocol(config)
    protocol.setEventCallback(Listener())
    val adoption = OfflineProtocolReferenceStorage.initializeMls(protocol, context, config.appId, config.profile)
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
    OfflineProtocolReferenceStorage.wipe(context, config.appId, config.profile)
}

// Verify a protocol-state backend before wiring it in (C11): green means
// supported. Run it off the main thread, and on a scratch instance: the suite
// blocks on writes of several megabytes.
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
