package consumer

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.util.Log
import com.offlineprotocol.InternetManager
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
