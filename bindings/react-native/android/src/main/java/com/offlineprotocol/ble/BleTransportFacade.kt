package com.offlineprotocol.ble

import android.Manifest
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.BatteryManager
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import androidx.core.content.ContextCompat
import com.offlineprotocol.BleDiscoveryBootstrapPolicy
import com.offlineprotocol.TransportException
import com.offlineprotocol.TransportManager
import com.offlineprotocol.TransportManagerListener
import com.offlineprotocol.TransportState
import com.offlineprotocol.optNullableString
import com.offlineprotocol.mesh.MeshAdvertisementData
import com.offlineprotocol.mesh.MeshController
import com.offlineprotocol.mesh.MeshController.ConnectionIntent
import com.offlineprotocol.mesh.MeshController.MeshRole
import uniffi.offline_protocol.OfflineProtocol
import android.bluetooth.BluetoothStatusCodes
import java.util.*
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ThreadLocalRandom
import java.util.concurrent.TimeUnit
import kotlin.math.min
import kotlin.math.roundToInt

private class LogThrottler(private val defaultIntervalMs: Long = 5000L) {
    private val timestamps = ConcurrentHashMap<String, Long>()

    fun shouldLog(key: String, intervalMs: Long = defaultIntervalMs, nowMs: Long = System.currentTimeMillis()): Boolean {
        val last = timestamps[key]
        if (last != null && nowMs - last < intervalMs) {
            return false
        }
        timestamps[key] = nowMs
        return true
    }
}

/**
 * Computes the effective per-peer ATT payload to flush to the Rust fragmenter
 * from the two link directions' known payloads.
 *
 * A single fragment stream must fit BOTH the central link we opened and the
 * peripheral/NOTIFY link the peer opened to us, so the result is the **minimum**
 * of whatever is known. The subtlety this captures: the notify link's MTU is only
 * observable via the GATT-server `onMtuChanged`, which is unreliable for the
 * server role and often never fires (an iOS central negotiates a smaller MTU on
 * the link it opens to us, or none). Without a peripheral value the `min()` would
 * collapse to the (larger) central payload, and a multi-fragment notify — an MLS
 * Welcome — would egress at central size, overflow the notify link, and be
 * silently truncated on air (the offline 1:1 Welcome-delivery stall).
 *
 * So when the peer has an active notify subscription ([notifySubscribed]) but no
 * observed peripheral payload, the peripheral term falls back to the conservative
 * [floor] (the 185-byte fragment cap every BLE link can carry) until a real value
 * arrives. A real observed payload always wins — [peripheralStaged] is consulted
 * first — so the floor never demotes a link whose MTU we actually know.
 *
 * Returns null only when neither direction is known AND the peer is not
 * notify-subscribed, signalling the caller to clear the Rust entry (the
 * fragmenter then reverts to its own floor). Pure and side-effect free so the
 * floor/min arithmetic is unit-testable without the Android BLE stack.
 */
internal fun computeEffectivePayload(
    central: Int?,
    peripheralStaged: Int?,
    notifySubscribed: Boolean,
    floor: Int,
): Int? {
    val peripheral = peripheralStaged ?: if (notifySubscribed) floor else null
    return listOfNotNull(central, peripheral).minOrNull()
}

/**
 * Resolves, by PEER IDENTITY, the subscribed peripheral-link address that
 * belongs to [deviceId] — or null if no notify-subscribed central maps to this
 * peer.
 *
 * This is the SINGLE device-scoped resolution shared by the per-peer MTU floor
 * ([BleTransportFacade.flushPeerMtu]) and the notify egress
 * ([BleTransportFacade.sendFragmentData]). Keeping one predicate is load-bearing:
 * the floor must be applied for exactly the peers the notify path can reach, or a
 * multi-fragment notify (an MLS Welcome) sized for the larger central link
 * overflows the unobserved notify link and is silently truncated on air. The two
 * links can use DIFFERENT BLE addresses for the same peer (iOS uses distinct
 * handles per direction), so we match by device id, not address.
 *
 * [resolveDeviceId] maps a subscribed address to the device id it registered
 * under ([MeshConnectionRegistry.deviceIdForAddress]); a subscribed address that
 * has not resolved to any device id (null) never matches. Pure and side-effect
 * free so the resolution is unit-testable without the Android BLE stack.
 */
internal fun resolveSubscribedAddress(
    deviceId: String,
    subscribedAddresses: Collection<String>,
    resolveDeviceId: (String) -> String?,
): String? = subscribedAddresses.firstOrNull { resolveDeviceId(it) == deviceId }

/**
 * BLE transport facade implementing [TransportManager] for Bluetooth Low
 * Energy communication. Ensures iOS ↔ Android cross-platform compatibility.
 *
 * The peripheral GATT server is delegated to [PeripheralGattServer] (which
 * attaches a CCCD descriptor to every notify characteristic and runs a
 * service-ready watchdog), advertising is delegated to [LeAdvertiser],
 * the outbound fragment backpressure queue lives in [OutboundFragmentQueue],
 * and connection bookkeeping lives in [MeshConnectionRegistry]. The
 * central-role path (scanning + the GATT client callback + mesh
 * orchestration) still lives in this class — it is the next slice of the
 * migration and its size is why this file is large.
 *
 * ### Why the central role is hand-rolled
 *
 * An earlier attempt on this branch depended on the Nordic Android-BLE-Library
 * (`no.nordicsemi.android:ble`) for op-queue serialisation and automatic CCCD
 * writes. That dependency was dropped before merge and central-role stayed
 * hand-rolled because:
 *
 *   1. The mesh's connection semantics — per-address link rather than
 *      per-peer, role flips via [MeshConnectionRegistry], provisional
 *      bootstrap connects issued before a device ID is known — did not
 *      map cleanly onto Nordic's per-peer `BleManager` lifecycle. A
 *      skeleton on this branch (`dea8138`) measured the wrapping cost
 *      and found it comparable to writing the callback directly, at
 *      which point the library stops paying for itself.
 *   2. The queue transitive pulls in a coroutine runtime and a logging
 *      transitive that we otherwise don't ship. This is a minor cost
 *      rather than a load-bearing argument, but it is not zero.
 *
 * The tradeoff we're accepting: Nordic has absorbed years of OEM-specific
 * Android BLE quirks that a hand-rolled callback will rediscover over
 * time. The known holes (CCCD write on subscribe, long-read offset,
 * binder-thread isolation, fragment keying) are now closed, but the
 * long-tail surface for stack-specific bugs on unseen devices is larger
 * here than it would be with Nordic. That is the cost of owning the
 * state machine; the upside is that every future fix lands in this
 * repo rather than behind a library version bump.
 *
 * If a future feature needs something Nordic provides cleanly (MTU
 * negotiation retries, indication handling, bonding-state serialisation),
 * revisit this — and extract a per-peer client abstraction first so the
 * glue surface is contained before the library comes back in.
 */
class BleTransportFacade(
    private val context: Context,
    // Thread-safe: OfflineProtocol uses Mutex/RwLock internally (see offline-protocol-uniffi)
    private val protocol: OfflineProtocol,
    private val deviceId: String,
    private val diagnosticEmitter: ((String, String, Map<String, Any?>) -> Unit)? = null
) : TransportManager {
    
    // MARK: - TransportManager Implementation
    
    override val transportId = "ble"
    override val transportName = "Bluetooth Low Energy"
    override var state: TransportState = TransportState.UNAVAILABLE
        private set
    override var listener: TransportManagerListener? = null
    
    // MARK: - BLE Constants (matching Rust core and iOS)
    
    companion object {
        private const val TAG = "BleTransportFacade"
        
        // UUIDs must match iOS and Rust core exactly
        private val SERVICE_UUID = UUID.fromString("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")
        private val MESSAGE_CHAR_UUID = UUID.fromString("6E400002-B5A3-F393-E0A9-E50E24DCCA9E")
        private val DEVICE_ID_CHAR_UUID = UUID.fromString("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")
        private val IDENTITY_CHAR_UUID = UUID.fromString("6E400004-B5A3-F393-E0A9-E50E24DCCA9E")
        private const val AD_TYPE_INCOMPLETE_128_BIT_SERVICE_UUIDS = 0x06
        private const val AD_TYPE_COMPLETE_128_BIT_SERVICE_UUIDS = 0x07
        private const val UUID_128_BIT_LENGTH_BYTES = 16
        private val SERVICE_UUID_LE_BYTES = uuidToLittleEndianBytes(SERVICE_UUID)
        
        // Fallback interval for fragment polling. Primary send path is event-driven
        // via onFragmentsAvailable(); this timer only catches edge cases.
        private const val FRAGMENT_POLL_INTERVAL_MS = 2000L
        private const val MAX_FRAGMENT_SIZE = 185
        /**
         * Hard cap on the number of fragments [drainAndSendFragments] will
         * pull from the Rust side in one tick. Above this we yield via
         * `bleHandler.post` and resume on the next loop iteration so a backlog
         * burst cannot monopolise the BLE looper.
         *
         * This used to be an ANR guard, back when the drain ran on the app's
         * main thread. It is not any more — see [bleLooper] — but the yield is
         * still what keeps a burst from starving the scan, advertising and
         * connection-monitor work that shares this looper.
         */
        private const val MAX_DRAIN_ITERATIONS_PER_CALL = 32
        private const val BACKPRESSURE_RETRY_MS = 50L
        /**
         * Ceiling of the backpressure retry ladder ([BackpressureRetryPolicy]).
         * Matched to [FRAGMENT_POLL_INTERVAL_MS] deliberately: past this rung
         * the fast retry is no longer buying anything the unconditional 2s
         * poller does not already provide, so there is no reason to keep
         * taking the protocol mutex more often than it does.
         */
        private const val BACKPRESSURE_RETRY_MAX_MS = 2_000L
        /**
         * Consecutive backpressure retries before the drain stops re-arming and
         * leaves the outbound queue to the polling floor. With the 50ms → 2s
         * ladder this sums to roughly 15 seconds of fast retrying, which clears
         * any transient stall many times over; a peer still stalled after it is
         * not going to be rescued by a sixteenth attempt, and continuing to
         * repost is what turns one half-open link into a 20Hz main-thread loop
         * contending the core mutex (OFF-2123).
         */
        private const val MAX_BACKPRESSURE_RETRY_ATTEMPTS = 12
        /** Max time the per-peer write gate ([writeInFlight]) stays closed
         *  before the next send is allowed regardless of completion callback.
         *
         *  The fast path is onCharacteristicWrite releasing the gate the
         *  instant the stack accepts the next write. But for
         *  WRITE_TYPE_NO_RESPONSE that callback is stack-dependent and on many
         *  devices never fires — observed on-device as the gate clearing only
         *  via this timeout. So this is NOT a rare-glitch watchdog; it is the
         *  steady-state inter-write pace whenever the callback is absent. Keep
         *  it small so a missing callback doesn't throttle throughput, but
         *  above the few ms a write actually occupies the stack so the next
         *  write doesn't out-run it into ERROR_GATT_WRITE_REQUEST_BUSY (201).
         *  An occasional 201 here still self-heals via the backpressure retry. */
        private const val WRITE_GATE_WATCHDOG_MS = 30L
        /** Per-peer gate hold for the peripheral INDICATE (notify) egress: the
         *  safety-net timeout before the next indication is allowed absent a
         *  completion callback.
         *
         *  Unlike the central WRITE_TYPE_NO_RESPONSE path, the notify path has a
         *  RELIABLE completion signal — the message characteristic is an
         *  INDICATION (ATT-confirmed), so onNotificationSent fires on the
         *  central's confirmation (real delivery) and that is the fast path that
         *  releases the gate. This watchdog is therefore a true fallback, not the
         *  steady-state pace, so it is deliberately LONGER than
         *  [WRITE_GATE_WATCHDOG_MS]: an indication confirmation needs at least one
         *  connection interval plus the central's processing (commonly 30–50ms+),
         *  and a 30ms watchdog would routinely pre-empt the confirmation, re-drive
         *  into a stack that rejects a second outstanding indication, and churn
         *  re-enqueues — partly defeating the per-fragment flow control INDICATE
         *  exists to provide. A genuinely lost confirmation costs at most this
         *  long per fragment (≈15 fragments stays well under the reassembly TTL
         *  and the mesh welcome-confirm timeout). */
        private const val NOTIFY_GATE_WATCHDOG_MS = 250L
        /** Minimum spacing between two completion-driven fragment sends to the
         *  same link.
         *
         *  The write gate's fast path ([onWriteCompleted]) drains the next
         *  fragment the instant the local stack signals completion
         *  (onCharacteristicWrite / onNotificationSent). But for a peripheral
         *  NOTIFY that completion fires when the notification leaves OUR
         *  controller, NOT when the central (e.g. an iOS device) has drained it
         *  from its receive buffer — so back-to-back notifies out-run a slower
         *  central and it silently drops fragments mid-burst. A small multi-
         *  fragment message (a few fragments) survives; a large one (an MLS
         *  Welcome, ~15 fragments) deterministically loses a fragment every
         *  pass and never reassembles. Spacing completion-driven sends by ~one
         *  BLE connection interval lets the peer drain between notifications.
         *  Tiny relative to a message's lifetime (15 fragments ≈ 0.3s) and
         *  well under the reassembly TTL. */
        private const val INTER_FRAGMENT_PACING_MS = 20L
        private const val CONNECTION_TIMEOUT_MS = 10000L
        private const val SCAN_WATCHDOG_INTERVAL_MS = 30000L // Match iOS timing
        private const val SCAN_WATCHDOG_HEARTBEAT_MS = 10000L
        /**
         * Retry cadence for re-arming BLE after a start could not get a scan
         * going. Doubles per consecutive failure and caps, so leaving Bluetooth
         * off overnight settles at two wakeups a minute rather than 8,600 of them,
         * while the first few retries stay fast enough that a quick toggle is
         * barely noticed. The cap is deliberately low: nothing else re-arms the
         * scan, so it is also the worst-case time this device stays deaf after
         * the user switches Bluetooth back on.
         */
        private const val BLE_RECOVERY_RETRY_MIN_MS = 10000L
        private const val BLE_RECOVERY_RETRY_MAX_MS = 30000L
        private const val MAX_CONNECTIONS_PER_DEVICE = 4
        /** Min interval between connection-monitor reconnect attempts per
         *  device. Reconnect backoff on disconnect is owned by
         *  [CentralGattClient] and lives there too. */
        private const val MIN_RECONNECT_INTERVAL_MS = 5_000L
        
        // Adaptive Scan Configuration
        /** Minimum RSSI to consider for connection (filter weak signals early) - matches iOS */
        private const val ADAPTIVE_MIN_RSSI = -85
        /** Absolute minimum RSSI below which we refuse to connect - matches iOS */
        private const val MINIMUM_RSSI_TO_CONNECT = -90
        /** Peer count threshold below which we process all advertisements */
        private const val ADAPTIVE_LOW_DENSITY_THRESHOLD = 10
        /** Peer count threshold above which we apply maximum throttling */
        private const val ADAPTIVE_HIGH_DENSITY_THRESHOLD = 50
        /** Maximum connection attempts per minute in dense networks */
        private const val ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE = 6
        /** Minimum interval between connection attempts to the same device (ms) */
        private const val ADAPTIVE_COOLDOWN_PER_DEVICE_MS = 30_000L
        /** Interval for updating visible peer count estimate (ms) */
        private const val ADAPTIVE_PEER_COUNT_WINDOW_MS = 5_000L
        /** Cooldown between provisional bootstrap attempts for unknown devices */
        private const val UNKNOWN_BOOTSTRAP_RATE_LIMIT_MS = 12_000L
        /** Minimum RSSI required for provisional bootstrap attempt */
        private const val UNKNOWN_BOOTSTRAP_MIN_RSSI = -75
        /** Stricter RSSI requirement when scan record is missing */
        private const val UNKNOWN_BOOTSTRAP_MIN_RSSI_NO_SCAN_RECORD = -68
        /** Max provisional bootstrap attempts per minute */
        private const val MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE = 4
        /** Proactive scan refresh interval even when discoveries are occurring (ms) */
        private const val PROACTIVE_SCAN_REFRESH_MS = 60_000L
        /** Force a complete BLE stack refresh periodically even when things seem healthy (ms) */
        private const val FORCED_BLE_REFRESH_MS = 120_000L
        /** Maximum consecutive scan restarts before resetting BLE adapter */
        private const val MAX_CONSECUTIVE_SCAN_RESTARTS = 3
        /** Backoff period after resetting BLE adapter (ms) */
        private const val ADAPTER_RESET_BACKOFF_MS = 45_000L
        /** Connection monitor interval for periodic reconnection attempts (ms) */
        private const val CONNECTION_MONITOR_INTERVAL_MS = 5_000L
        /** Initial aggressive discovery phase duration (ms) - more frequent scanning initially */
        private const val AGGRESSIVE_DISCOVERY_PHASE_MS = 30_000L
        /** TTL for negative cache entries of verified non-mesh devices (ms) */
        private const val NON_MESH_CACHE_TTL_MS = 300_000L // 5 minutes
        /** Initial backoff when updateSignedIdentity fails (MLS not ready etc.) */
        private const val IDENTITY_REFRESH_MIN_BACKOFF_MS = 500L
        /** Cap on the identity-refresh retry backoff */
        private const val IDENTITY_REFRESH_MAX_BACKOFF_MS = 10_000L
        /**
         * Hard cap on consecutive identity refresh retries before we declare
         * the identity cache permanently broken and stop rescheduling. With
         * the 500ms → 10s backoff, the schedule sums to roughly five minutes
         * of recovery time before we surface a terminal diagnostic. Without
         * the cap a permanently broken MLS init silently retries forever and
         * every central read of the identity characteristic returns
         * GATT_FAILURE with no observability.
         */
        private const val MAX_IDENTITY_REFRESH_ATTEMPTS = 30

        /**
         * The single looper every BLE operation runs on.
         *
         * This used to be the app's main looper, and nothing here ever needed
         * it to be: the whole `ble/` package touches no UI — no View, no
         * Toast, no Activity — and used main purely as a "one thread, ordered
         * posts" primitive. What that cost is the reason OFF-2123 exists.
         * Every fragment drain calls into UniFFI, which serialises on one
         * global protocol mutex held across MLS work and AndroidKeyStore-backed
         * storage callbacks; on main, waiting for that mutex is charged to the
         * thread Android watches for ANRs. Moving to a private looper keeps the
         * serialization the design depends on and takes the app's
         * responsiveness out of its blast radius.
         *
         * This is also what the platform actually recommends: Android's BLE
         * APIs do not require the main thread, and the canonical guidance is a
         * dedicated handler plus a command queue — which is precisely the shape
         * this facade already had, aimed at the wrong thread.
         *
         * Process-wide and never quit, deliberately. The main looper it
         * replaces was never quit either, so this preserves the old contract
         * exactly: no teardown path can strand a pending post on a dead looper,
         * and a facade rebuilt after `stop()` inherits the same ordered queue
         * rather than racing a fresh one. One idle thread is the entire cost.
         */
        private val bleThread: HandlerThread by lazy {
            HandlerThread("offline-ble").apply { start() }
        }

        /**
         * How long a main-thread caller will wait for the BLE thread before
         * giving up on it. Far below the 5s input-dispatch ANR budget, and far
         * above a healthy queue turnaround — see [runOnBleThreadSync].
         */
        private const val MAIN_THREAD_SYNC_TIMEOUT_MS = 1_000L

        internal val bleLooper: Looper get() = bleThread.looper

        /** Runtime guard for the collaborators that share [bleLooper]. */
        internal fun assertOnBleLooper(reason: String) {
            check(Looper.myLooper() == bleLooper) {
                "$reason must run on the BLE thread (was ${Thread.currentThread().name})"
            }
        }

        private fun uuidToLittleEndianBytes(uuid: UUID): ByteArray {
            val hexUuid = uuid.toString().uppercase().replace("-", "")
            val bigEndianBytes = hexUuid.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
            return bigEndianBytes.reversedArray()
        }
    }
    
    // MARK: - Properties
    
    private val bluetoothManager: BluetoothManager = 
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val bluetoothAdapter: BluetoothAdapter? = bluetoothManager.adapter
    
    // Scanner components
    private var bluetoothLeScanner: BluetoothLeScanner? = null
    private var scanCallback: ScanCallback? = null
    // BLE-thread only today: every reader and writer runs on bleHandler,
    // onScanFailed included — it reposts before touching this. @Volatile is
    // kept deliberately rather than as a live requirement; this flag has been
    // written from a binder thread before, and a volatile read on a field
    // touched a few times a minute costs nothing next to re-deriving the
    // threading argument the next time a callback path grows.
    @Volatile
    private var isScanning = false
    // Set only when scan startup observes the adapter off. Generic scan
    // failures must not rebuild the peripheral or churn a healthy advertiser.
    private var adapterWasOff = false
    // Last availability state successfully delivered to the Rust core. Null
    // means this facade has not reported a state in its current session;
    // [stopUnsafe] clears it, because one facade instance is reused across
    // disable/enable and the dedup below must not span that boundary.
    private var reportedBleAvailable: Boolean? = null
    
    // Advertiser component (delegates to LeAdvertiser).
    // Lazy so its construction sees bleHandler / logThrottler / peripheralGattServer
    // which are declared later in this file.
    private val leAdvertiser: LeAdvertiser by lazy(LazyThreadSafetyMode.NONE) {
        LeAdvertiser(
            bleHandler = bleHandler,
            host = object : LeAdvertiser.Host {
                override fun isGattServerReady(): Boolean = peripheralGattServer?.isReady == true
                override fun buildAdvertiseData() = this@BleTransportFacade.buildAdvertiseData()
                override fun buildScanResponse() = this@BleTransportFacade.buildScanResponse()
                override fun refreshPublishedIdentity() { updateSignedIdentity() }
                override fun shouldLog(key: String, intervalMs: Long) =
                    logThrottler.shouldLog(key, intervalMs = intervalMs)
            },
            diagnosticEmitter = { level, message, ctx -> emitDiagnostic(level, message, ctx) },
        )
    }

    // Central-role GATT client callback + per-address handshake state.
    // Lazy so construction sees bleHandler / connections / pendingInbound /
    // meshController, all declared lower in this file. The Host
    // implementation below is the narrow, explicit re-entry surface the
    // client uses to call back into the facade.
    private val centralClient: CentralGattClient by lazy(LazyThreadSafetyMode.NONE) {
        CentralGattClient(
            bleHandler = bleHandler,
            serviceUuid = SERVICE_UUID,
            messageCharUuid = MESSAGE_CHAR_UUID,
            deviceIdCharUuid = DEVICE_ID_CHAR_UUID,
            identityCharUuid = IDENTITY_CHAR_UUID,
            host = object : CentralGattClient.Host {
                override val protocol: OfflineProtocol get() = this@BleTransportFacade.protocol
                override val connections: MeshConnectionRegistry get() = this@BleTransportFacade.connections
                override val pendingInbound: InboundFragmentBuffer get() = this@BleTransportFacade.pendingInbound
                override val outboundQueue: OutboundFragmentQueue get() = this@BleTransportFacade.outboundQueue
                override val meshController: MeshController get() = this@BleTransportFacade.meshController
                override val bluetoothAdapter: BluetoothAdapter? get() = this@BleTransportFacade.bluetoothAdapter
                override val selfDeviceId: String get() = deviceId

                override fun isShuttingDown(): Boolean = shuttingDown
                override fun isRunning(): Boolean = state == TransportState.RUNNING

                override fun rssiFor(address: String): Short? = lastSeenRssi[address]
                override fun clearRssi(address: String) { lastSeenRssi.remove(address) }
                override fun markNonMeshDevice(address: String) {
                    verifiedNonMeshDevices[address] = System.currentTimeMillis()
                }

                override fun refreshAdvertising(reason: String) =
                    this@BleTransportFacade.refreshAdvertising(reason)
                override fun refreshSelfMetrics() =
                    this@BleTransportFacade.refreshSelfMetrics()
                override fun maybeHandleRebalance(trigger: String) =
                    this@BleTransportFacade.maybeHandleRebalance(trigger)
                override fun learnRouteFromMessage(
                    messageJson: String,
                    neighborId: String,
                    neighborAddress: String?,
                ) = this@BleTransportFacade.learnRouteFromMessage(messageJson, neighborId, neighborAddress)
                override fun drainAndSendFragments() =
                    this@BleTransportFacade.drainAndSendFragments()
                override fun onWriteCompleted(address: String) =
                    this@BleTransportFacade.onWriteCompleted(address)
                override fun handleInboundFragment(address: String, data: ByteArray) =
                    this@BleTransportFacade.handleReceivedData(data, address)
                override fun connectToDevice(device: BluetoothDevice) =
                    this@BleTransportFacade.connectToDevice(device)

                override fun onPeerMtuNegotiated(address: String, maxPayload: Int) {
                    // Called from the binder GATT callback thread. Repost
                    // to the BLE thread so we can touch [peerMaxPayloads] and the
                    // connection registry without racing the handshake
                    // state machine, and so any UniFFI call made from the
                    // already-resolved branch below lands on the BLE thread.
                    bleHandler.post {
                        if (shuttingDown) return@post
                        assertOnBleThread("onPeerMtuNegotiated.stage")
                        // Re-check after the main-hop: by the time we
                        // run, teardown may have completed (disconnect,
                        // give-up, stop) and removed the gatt client
                        // from the registry. Staging in that window
                        // would leak an entry in [peerMaxPayloads] that
                        // no subsequent teardown path would clear, and
                        // a reconnect on the same BLE address could
                        // then observe a stale value from the previous
                        // session. Drop the stage silently.
                        if (connections.getGatt(address) == null) {
                            return@post
                        }
                        peerMaxPayloads[address] = maxPayload
                        // If the reverse GATT read has already landed and
                        // resolved the device id for this address, flush
                        // immediately instead of waiting for a subsequent
                        // onDeviceIdResolved — this covers the case where
                        // the MTU ack arrives *after* the device-id read
                        // (theoretically possible if a peer renegotiates
                        // MTU mid-link, though not in the normal chain).
                        val deviceId = connections.deviceIdForAddress(address)
                        if (deviceId != null) {
                            flushPeerMtu(address, deviceId)
                        }
                    }
                }

                override fun onDeviceIdResolved(address: String, deviceId: String) {
                    // Already on the BLE thread — handleDeviceIdRead runs on the
                    // BLE thread, so no hop is needed.
                    if (shuttingDown) return
                    flushPeerMtu(address, deviceId)
                }

                override fun onPeerGivenUp(address: String, peerId: String) {
                    // Called on the BLE thread from finalizeGivenUpPeer, which has
                    // already invoked `protocol.blePeerLost(peerId)` —
                    // that drops the Rust-side per-peer MTU entry inside
                    // `on_peer_lost`. All that remains is the facade-side
                    // staged entry keyed by BLE address (non-empty in the
                    // edge case where `onPeerMtuNegotiated` landed but
                    // the device-id read never completed) plus the
                    // per-device MTU slots, both cleared below.
                    //
                    // Asymmetry (by design): this drops the peer's MTU state
                    // wholesale even if a peripheral/notify link to the same
                    // peer is still alive — whether a central give-up should
                    // imply peer loss is a protocol-level question left out of
                    // scope here. A surviving link self-heals: its next inbound
                    // fragment re-stages the MTU via onPeripheralMtuNegotiated.
                    if (shuttingDown) return
                    dropStagedPeerMtu(address, peerId)
                }
            },
            diagnosticEmitter = { level, message, ctx -> emitDiagnostic(level, message, ctx) },
        )
    }

    // GATT Server (peripheral role). Delegated to [PeripheralGattServer] so
    // that the NOTIFY characteristic always carries a CCCD descriptor,
    // descriptor writes are acked, service registration has a watchdog, and
    // long reads honour offsets.
    private var peripheralGattServer: PeripheralGattServer? = null
    
    // Cached signed identity data for serving via GATT
    // Read by provideIdentityBytes() on the binder thread; written by
    // updateSignedIdentity() on the BLE thread / binder. @Volatile so the latest
    // reference is visible to binder-thread readers.
    @Volatile
    private var cachedSignedIdentity: com.offlineprotocol.mesh.SignedIdentityData? = null

    // This device's derived address (`off1…`) — what DEVICE_ID serves.
    // Same threading contract as [cachedSignedIdentity]: read by
    // provideDeviceIdBytes() on the binder thread, written by
    // updateSignedIdentity() on the BLE thread. @Volatile so binder-thread readers see
    // the latest value. Null until MLS is initialized, which is what makes
    // the peripheral fail closed rather than advertise an unprovable id.
    @Volatile
    private var cachedLocalAddress: String? = null

    // Backoff state for updateSignedIdentity retries when MLS is not yet
    // initialized or signing fails. BLE-thread only.
    private var identityRefreshRetryScheduled: Boolean = false
    private var identityRefreshRetryDelayMs: Long = IDENTITY_REFRESH_MIN_BACKOFF_MS
    private var identityRefreshAttempts: Int = 0
    /**
     * Latched once the retry budget is exhausted. While set, every call to
     * [ensureIdentityRefreshScheduled] is a no-op so the GATT-server binder
     * thread can't keep posting refresh requests on every central read.
     * Cleared on [stop] so a fresh [start] gets a fresh budget.
     */
    private var identityRefreshGivenUp: Boolean = false
    
    // Connection registry keeps track of client/server links and desired roles.
    private val connections = MeshConnectionRegistry()
    private val lastSeenRssi = ConcurrentHashMap<String, Short>()

    // Per-address negotiated ATT payload for the CENTRAL link — the
    // connection WE opened to the peer's GATT server. Populated from
    // [CentralGattClient.Host.onPeerMtuNegotiated] and flushed once
    // [CentralGattClient.Host.onDeviceIdResolved] maps the address to a stable
    // device id. The Rust core stores ONE MTU per peer, so the value flushed
    // is min(this, the peripheral-link payload below) — see [flushPeerMtu].
    // Entries persist until link teardown (they are inputs to the min, not
    // one-shot), and are cleared in [dropStagedPeerMtu] / on stop. BLE-thread
    // only — every access is guarded by [assertOnBleThread], and the type is a
    // plain `HashMap` to make the discipline visible.
    private val peerMaxPayloads = HashMap<String, Int>()
    // Per-address negotiated ATT payload for the PERIPHERAL/NOTIFY link — the
    // connection the peer opened to OUR GATT server, reported by
    // [PeripheralGattServer.Listener.onPeripheralMtuNegotiated]. A multi-
    // fragment message (e.g. an MLS Welcome) sized for the central link but
    // egressed as a peripheral notify would overflow this link and be
    // truncated/dropped on air; folding this into the per-peer MTU via min()
    // is the offline-convergence fix. Cleared when the peripheral link drops
    // (in [handleCentralDisconnectedOnBleThread]) so the central MTU is restored.
    // BLE-thread only.
    private val peripheralMaxPayloads = HashMap<String, Int>()
    // Per-DEVICE-ID negotiated ATT payloads, one slot per direction. A peer can
    // be reachable over two links with DIFFERENT BLE addresses (iOS uses distinct
    // connection handles per direction), so the central and peripheral payloads
    // must be combined by device identity, not by a single address — otherwise a
    // late renegotiation on one link would flush an MTU unbounded by the other and
    // the notify egress overflows it (the offline 1:1 Welcome stall this fix
    // targets). The per-address maps above are the deferral buffer for a payload
    // negotiated before the device id resolves; [flushPeerMtu] promotes them into
    // these per-device slots and mins across THESE. Cleared per-direction on that
    // link's teardown. BLE-thread only.
    private val centralPayloadByDevice = HashMap<String, Int>()
    private val peripheralPayloadByDevice = HashMap<String, Int>()
    private val discoveryLogTimestamps = ConcurrentHashMap<String, Long>()
    @Volatile private var lastDiscoveryAt: Long = 0L

    // Barrier that gates binder-thread GATT callbacks from mutating shared
    // state during teardown. Raised synchronously on the BLE thread at the
    // top of [stopUnsafe] before any `clear()` call, and left raised until
    // a subsequent [startUnsafe] explicitly lowers it. Callbacks on other
    // threads read this via [CentralGattClient.Host.isShuttingDown] before
    // touching `connections`, the link-ready set, or `pendingInbound`, so a
    // late delivery can no longer observe half-cleared state.
    @Volatile private var shuttingDown: Boolean = false

    private val logThrottler = LogThrottler()
    
    private data class MeshObservation(val advertisement: MeshAdvertisementData, val rssi: Int?, val timestamp: Long)
    // Single address-keyed inbound buffer used by both the GATT-client path
    // (the central-side notify callback inside [CentralGattClient]) and the
    // GATT-server path (central → peripheral writes delivered through
    // [PeripheralGattServer.Listener.onInboundFragment]). Entries are queued
    // while a peer's stable device ID is still being resolved via a reverse
    // GATT read, and drained by the client's device-id-read handler once
    // that read lands. Using the connection-specific address as the key is
    // RPA-safe: the address is stable for the lifetime of a single LL
    // connection on both sides, even if the peer's advertised MAC rotates
    // outside of it.
    //
    // All mutating access is BLE-thread only; the contract is enforced at
    // runtime inside [InboundFragmentBuffer] via the same pattern used by
    // [OutboundFragmentQueue]. Earlier revisions of this branch held an
    // explicit `synchronized(pendingFragmentsLock)` block around HashMap
    // mutations, but every call site now runs on the BLE thread (binder
    // callbacks post here via bleHandler), so a single-threaded dispatcher
    // makes the lock unnecessary — the runtime BLE-thread check replaces it.
    private val pendingInbound = InboundFragmentBuffer(
        onDropped = { address, reason, count ->
            when (reason) {
                InboundFragmentBuffer.DropReason.CAPPED_PER_PEER -> {
                    if (logThrottler.shouldLog("pending_inbound_capped_$address", intervalMs = 10_000)) {
                        Log.w(
                            TAG,
                            "Pending inbound fragment buffer capped for $address, dropped whole queue (count=$count)",
                        )
                        emitDiagnostic(
                            "warning",
                            "Pending inbound fragment buffer capped",
                            mapOf(
                                "address" to address,
                                "dropped" to count,
                                "max" to InboundFragmentBuffer.DEFAULT_MAX_PER_PEER,
                            ),
                        )
                    }
                }
                InboundFragmentBuffer.DropReason.CAPPED_PEERS -> {
                    if (logThrottler.shouldLog("pending_inbound_peer_cap_evict", intervalMs = 10_000)) {
                        Log.w(
                            TAG,
                            "Pending-fragment peer cap hit; evicting $address (count=$count)",
                        )
                        emitDiagnostic(
                            "warning",
                            "Pending-fragment peer cap evicted buffer",
                            mapOf(
                                "victim" to address,
                                "dropped" to count,
                                "cap" to InboundFragmentBuffer.DEFAULT_MAX_PEERS,
                            ),
                        )
                    }
                }
                InboundFragmentBuffer.DropReason.EXPIRED -> {
                    if (logThrottler.shouldLog("pending_inbound_expired_$address", intervalMs = 10_000)) {
                        Log.w(TAG, "Dropped $count expired pending inbound fragments for $address")
                        emitDiagnostic(
                            "warning",
                            "Pending inbound fragments expired",
                            mapOf("address" to address, "expired" to count),
                        )
                    }
                }
            }
        },
    )
    private val LOAD_SATURATION_COUNT = 20
    private val MESH_OBSERVATION_TTL_MS = 120_000L

    private val meshController = MeshController(deviceId)
    
    // Adaptive scan state
    /** Timestamps of recent peripheral discoveries for density estimation */
    private val recentDiscoveryTimestamps = Collections.synchronizedList(mutableListOf<Long>())
    /** Last connection attempt timestamps per device for rate limiting */
    private val deviceConnectionAttempts = ConcurrentHashMap<String, Long>()
    /** Global connection attempts in the last minute for rate limiting */
    private val globalConnectionAttempts = Collections.synchronizedList(mutableListOf<Long>())
    /** Current estimated visible peer count */
    @Volatile private var estimatedVisiblePeerCount: Int = 0
    /** Last time we updated the peer count estimate */
    @Volatile private var lastPeerCountUpdate: Long = 0L
    @Volatile private var lastMeshAdvertisement: MeshAdvertisementData? = null
    /** Last time we proactively refreshed the scan */
    @Volatile private var lastProactiveScanRefresh: Long = 0L
    /** Last time we performed a forced BLE refresh */
    @Volatile private var lastForcedBleRefresh: Long = 0L
    /** Rate limiter for provisional unknown-device bootstrap attempts */
    private val unknownBootstrapAttempts = ConcurrentHashMap<String, Long>()
    /** Tracks recently seen advertisements to avoid duplicate processing (hash, timestamp) */
    private data class AdvertisementCacheEntry(val hash: Int, val timestamp: Long)
    private val recentAdvertisementHashes = ConcurrentHashMap<String, AdvertisementCacheEntry>()
    /** Negative cache: devices verified via GATT as non-mesh (address -> timestamp) */
    private val verifiedNonMeshDevices = ConcurrentHashMap<String, Long>()
    /** Counter for consecutive scan restarts without discoveries */
    @Volatile private var scanRestartCount = 0
    /** Last time we reset the BLE adapter */
    @Volatile private var lastAdapterReset: Long = 0L
    /** Connection monitor runnable for periodic reconnection attempts */
    private var connectionMonitorRunnable: Runnable? = null
    
    // Fragment polling. Runs on the private BLE looper, not the app's main
    // thread — see [bleLooper] for why.
    private val bleHandler = Handler(bleLooper)

    /**
     * Capped backoff for the outbound drain's self-rearm. See
     * [BackpressureRetryPolicy] for why a flat repost was unsafe.
     */
    private val backpressureRetry = BackpressureRetryPolicy(
        handler = bleHandler,
        task = Runnable {
            if (state == TransportState.RUNNING) {
                drainAndSendFragments()
            }
        },
        minDelayMs = BACKPRESSURE_RETRY_MS,
        maxDelayMs = BACKPRESSURE_RETRY_MAX_MS,
        maxConsecutiveAttempts = MAX_BACKPRESSURE_RETRY_ATTEMPTS,
    )

    /** The BLE thread did not answer a main-thread caller in time. */
    internal class MainThreadSyncTimeout :
        RuntimeException("BLE thread did not respond within ${MAIN_THREAD_SYNC_TIMEOUT_MS}ms")

    /**
     * Run [action] on the BLE thread and wait for it.
     *
     * The wait is unbounded for every caller except the app's main thread, and
     * that exception is load-bearing rather than defensive. The BLE thread can
     * legitimately sit for seconds inside a UniFFI call waiting on the core
     * protocol mutex; blocking main on it would rebuild the exact ANR this
     * facade was moved off main to escape (OFF-2123), just through the
     * lifecycle door instead of the fragment drain.
     *
     * No current caller reaches here on main — `start`/`stop`/`pause`/`resume`
     * arrive on React Native's native-modules thread, and none of the
     * `LifecycleEventListener` callbacks touch this transport — but which
     * thread RN invokes `invalidate()` on is a framework detail that has
     * already changed once. So the guarantee is enforced here rather than
     * assumed at every call site.
     *
     * On expiry the work is not cancelled: it still runs on the BLE thread, we
     * simply stop waiting for it, and the caller gets [MainThreadSyncTimeout].
     * Every action routed through here (`startUnsafe`, `stopUnsafe`,
     * `pauseUnsafe`, `resumeUnsafe`) is self-contained and idempotent, so
     * completing late is safe. Tolerating that throw is what the caller owes:
     * every main-reachable lifecycle path runs its transport stops through
     * `TeardownSequence`, which records a throwing step and carries on, so a
     * timeout costs one late-completing action and never a skipped teardown —
     * strictly better than an ANR.
     */
    private fun <T> runOnBleThreadSync(action: () -> T): T {
        if (Looper.myLooper() == bleLooper) {
            return action()
        }

        val onMainThread = Looper.myLooper() == Looper.getMainLooper()
        val latch = CountDownLatch(1)
        var outcome: Result<T>? = null
        bleHandler.post {
            outcome = try {
                Result.success(action())
            } catch (t: Throwable) {
                Result.failure(t)
            }
            latch.countDown()
        }

        try {
            if (onMainThread) {
                if (!latch.await(MAIN_THREAD_SYNC_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    Log.w(
                        TAG,
                        "BLE thread did not answer a main-thread caller within " +
                            "${MAIN_THREAD_SYNC_TIMEOUT_MS}ms; continuing without it",
                    )
                    emitDiagnostic(
                        "warning",
                        "BLE thread sync timed out on main",
                        mapOf("timeoutMs" to MAIN_THREAD_SYNC_TIMEOUT_MS),
                    )
                    throw MainThreadSyncTimeout()
                }
            } else {
                latch.await()
            }
        } catch (ie: InterruptedException) {
            Thread.currentThread().interrupt()
            throw RuntimeException("Interrupted while executing on BLE thread", ie)
        }

        return outcome!!.getOrThrow()
    }

    // Async variant: runs inline when already on the BLE thread, otherwise
    // posts. Used by BLE callbacks that must mutate BLE-thread-only state.
    private fun runOnBleThread(action: () -> Unit) {
        if (Looper.myLooper() == bleLooper) {
            action()
        } else {
            bleHandler.post(action)
        }
    }

    // Runtime contract check. Used at the top of every function that reads
    // or mutates state with a documented "BLE thread only" invariant. The
    // invariants used to live only in comments and leaked multiple times on
    // this branch; promoting them to runtime checks makes the next drift
    // fail loud instead of silent. [reason] is included in the message so
    // crash reports can identify the offending call site.
    private fun assertOnBleThread(reason: String) = assertOnBleLooper(reason)

    private val fragmentPollingRunnable = object : Runnable {
        override fun run() {
            pollAndSendFragments()
            if (state == TransportState.RUNNING) {
                bleHandler.postDelayed(this, FRAGMENT_POLL_INTERVAL_MS)
            }
        }
    }
    
    // Gradient routing cleanup
    private val ROUTING_CLEANUP_INTERVAL_MS = 30_000L
    private val routingCleanupRunnable = object : Runnable {
        override fun run() {
            protocol.cleanupExpiredRoutes()
            // Evict stale inbound pending fragments independent of traffic.
            // Without this, a peer that connects, queues fragments while its
            // device ID is being resolved, then goes silent will leak those
            // fragments until another peer's fragment triggers the eviction
            // sweep inside handleReceivedData.
            pendingInbound.evictExpired()
            if (state == TransportState.RUNNING) {
                bleHandler.postDelayed(this, ROUTING_CLEANUP_INTERVAL_MS)
            }
        }
    }
    
    // Outbound fragment backpressure queue. All mutating calls must run on
    // the BLE thread; the queue enforces this at runtime so the invariant
    // cannot silently drift. `totalCount` / `recipientIds` / `recipientCount`
    // are safe from any thread and are used by off-thread diagnostic paths.
    private val outboundQueue = OutboundFragmentQueue(
        onDropped = { recipientId, reason, count ->
            when (reason) {
                OutboundFragmentQueue.DropReason.CAPPED -> {
                    Log.w(
                        TAG,
                        "Pending outbound fragment queue capped for $recipientId, dropping oldest " +
                            "(max=${OutboundFragmentQueue.DEFAULT_MAX_PER_PEER})",
                    )
                }
                OutboundFragmentQueue.DropReason.EXPIRED -> {
                    if (logThrottler.shouldLog("fragments_expired_$recipientId", intervalMs = 10000)) {
                        Log.w(TAG, "Dropped $count expired outbound fragments for $recipientId")
                        emitDiagnostic(
                            "warning",
                            "Outbound fragments expired",
                            mapOf(
                                "recipientId" to recipientId,
                                "expired" to count,
                            ),
                        )
                    }
                }
            }
        },
    )
    private val lastSeenMeshAdvertisements = ConcurrentHashMap<String, MeshObservation>()
    private var transportStartAt: Long = 0L

    private val scanWatchdogRunnable = object : Runnable {
        override fun run() {
            if (!isScanning) {
                return
            }
            val now = System.currentTimeMillis()
            val idleMs = now - lastDiscoveryAt
            if (idleMs >= SCAN_WATCHDOG_INTERVAL_MS) {
                if (logThrottler.shouldLog("scan_watchdog", intervalMs = SCAN_WATCHDOG_INTERVAL_MS)) {
                    Log.w(TAG, "Restarting BLE scan after ${idleMs}ms of inactivity")
                    emitDiagnostic("warning", "Restarting BLE scan due to inactivity", mapOf("idleMs" to idleMs))
                }
                scanRestartCount++
                restartScanning("watchdog")
                evaluateBleHealthAfterRestart()
                return
            }
            bleHandler.postDelayed(this, SCAN_WATCHDOG_HEARTBEAT_MS)
        }
    }

    /**
     * The one path back to a live scan after the platform refuses a start.
     *
     * Every other self-healing mechanism in this class — the scan watchdog, the
     * connection monitor, the proactive and forced refreshes, and the adapter
     * reset they escalate to — hangs off an active scan. So a start that cannot
     * proceed leaves nothing on the handler to try again: the transport keeps
     * reporting RUNNING while the mesh is deaf, until the app happens to call
     * resume() or restart the transport. Re-arming the scan is what puts that
     * whole chain back in motion, which is why this runnable only has to get
     * scanning going again rather than rebuild the stack itself.
     *
     * Rescheduled from [startScanning] for as long as the refusal persists, and
     * cancelled by [stopScanning] so a paused or stopped transport cannot bring
     * scanning back behind the app's back. When [adapterWasOff] is set, the
     * first successful scan start also rebuilds the platform-owned GATT server;
     * advertising remains deferred until that service is registered again.
     *
     * This does not escalate:
     * [evaluateBleHealthAfterRestart], the stack rebuild that repeated scan
     * stalls climb to, hangs off the scan watchdog and so is unreachable while
     * there is no live scan to watch. That is the right trade for an adapter
     * that is simply off — there is nothing to rebuild until it comes back —
     * but it does mean a genuinely wedged stack is retried at the cap rather
     * than escalated.
    */
    private val bleRecoveryRunnable = object : Runnable {
        override fun run() {
            if (state != TransportState.RUNNING || (isScanning && !adapterWasOff)) {
                return
            }
            // Re-acquire the scanner before retrying. Not because the cached
            // instance goes stale — the platform hands back a per-adapter
            // singleton, not a fresh object per session — but because
            // [evaluateBleHealthAfterRestart] assigns whatever
            // getBluetoothLeScanner() returns, which is null while the adapter
            // is down, and nothing else re-reads it afterwards. Only overwrite
            // on a non-null read, so a working cached scanner is never lost.
            bluetoothAdapter?.bluetoothLeScanner?.let { bluetoothLeScanner = it }
            // Same for the advertiser, which that path re-attaches from the
            // same nullable read and so can have left detached.
            val recoveredAdvertiser = bluetoothAdapter?.bluetoothLeAdvertiser
            recoveredAdvertiser?.let { leAdvertiser.attachAdvertiser(it) }
            var scanStartFailure: Exception? = null
            // Whether [startScanning] below already armed the next attempt. A
            // start that lands while the adapter-off latch is still raised
            // re-arms through onScanStarted, so the peripheral repair further
            // down must not arm a second time: schedule() climbs the ladder on
            // every call, and arming twice per attempt makes each retry wait a
            // rung longer than the ladder claims and hit the cap early.
            var recoveryArmed = false
            if (!isScanning) {
                // Scan first, and let nothing that can throw sit upstream of it:
                // run() is a bare handler post, so an escape both takes the host
                // app down and leaves nothing pending. startScanning re-arms
                // every handled refusal but rethrows SecurityException.
                try {
                    startScanning("adapter_recovery")
                } catch (e: Exception) {
                    scanStartFailure = e
                    if (!isScanning) {
                        // Arm first: a diagnostic emitter is app code and may
                        // throw. There is no caller above this handler post.
                        scheduleBleRecovery()
                        Log.e(TAG, "BLE recovery scan attempt failed", e)
                        emitDiagnostic("error", "BLE recovery scan attempt failed", mapOf(
                            "exception" to e.javaClass.simpleName,
                            "message" to (e.message ?: "unknown"),
                        ))
                        return
                    }
                }
                // Reaching here means startScan returned. onScanStarted armed
                // the follow-up iff the adapter-off latch was raised, which is
                // exactly the condition the repair below runs under.
                recoveryArmed = isScanning && adapterWasOff
            }
            if (!isScanning) return

            if (adapterWasOff) {
                // Scanner and advertiser availability do not become visible
                // atomically on every stack. Keep the adapter-recovery episode
                // alive until there is an advertiser to attach; otherwise a
                // successful scan would make the active-scan guard swallow the
                // only remaining path that can restore discoverability.
                if (recoveredAdvertiser == null) {
                    if (!recoveryArmed) scheduleBleRecovery()
                    return
                }
                try {
                    // Android destroys both registrations while the adapter is
                    // off without updating either wrapper's local state. Drop
                    // the stale advertising gate, replace the GATT server, and
                    // latch advertising behind the new service-ready callback.
                    stopAdvertising()
                    check(setupGattServer()) { "GATT server setup did not start" }
                    startAdvertising("adapter_recovery")
                    adapterWasOff = false
                    // The episode is over: drop the follow-up onScanStarted
                    // armed above — its guard would no-op on it anyway — and
                    // put the ladder back on its bottom rung so the next outage
                    // is retried fast rather than at the cap. handleScanResult
                    // cannot do this for us on a device with no peers in range.
                    cancelBleRecovery()
                } catch (e: Exception) {
                    // Keep the scan. The entry guard above re-enters with a
                    // live scan while [adapterWasOff] is raised, so the next
                    // attempt retries the peripheral repair on its own; tearing
                    // the scan down here buys that retry nothing, and against a
                    // persistently failing repair it would cost a discovery gap
                    // and a rehydrate reconnect burst every single cycle, for as
                    // long as the transport runs. Arm before the emitters below,
                    // which are app code and may throw.
                    if (!recoveryArmed) scheduleBleRecovery()
                    // The scan survived, so the central role — discovery and
                    // outbound connections — works; only discoverability is
                    // still broken, and the retry armed above owns it. Report
                    // on the same basis [startUnsafe] does when setupGattServer
                    // fails there, rather than leaving the core convinced BLE
                    // is unusable for as long as the repair keeps failing.
                    reportBleAvailability(true, "recovery_scan_only")
                    Log.e(TAG, "BLE adapter recovery failed", e)
                    emitDiagnostic("error", "BLE adapter recovery failed", mapOf(
                        "exception" to e.javaClass.simpleName,
                        "message" to (e.message ?: "unknown"),
                    ))
                    return
                }
            }

            reportBleAvailability(true, "recovery")
            scanStartFailure?.let { e ->
                // startScanning can throw from app diagnostics after the
                // framework accepted the scan. Finish adapter repair first;
                // otherwise the next retry returns at the active-scan guard and
                // leaves the dead peripheral registration untouched.
                Log.e(TAG, "BLE recovery scan startup reported an error", e)
                emitDiagnostic("error", "BLE recovery scan startup reported an error", mapOf(
                    "exception" to e.javaClass.simpleName,
                    "message" to (e.message ?: "unknown"),
                ))
            }
        }
    }

    private val bleRecoveryScheduler by lazy(LazyThreadSafetyMode.NONE) {
        BleRecoveryScheduler(
            handler = bleHandler,
            task = bleRecoveryRunnable,
            minDelayMs = BLE_RECOVERY_RETRY_MIN_MS,
            maxDelayMs = BLE_RECOVERY_RETRY_MAX_MS,
        )
    }

    /**
     * Evaluates BLE stack health after consecutive restarts and resets adapter if needed.
     * This mirrors iOS's evaluateCentralHealthAfterRestart mechanism.
     */
    private fun evaluateBleHealthAfterRestart() {
        if (scanRestartCount < MAX_CONSECUTIVE_SCAN_RESTARTS) {
            return
        }
        
        val now = System.currentTimeMillis()
        if (lastAdapterReset > 0 && now - lastAdapterReset < ADAPTER_RESET_BACKOFF_MS) {
            return
        }
        
        Log.w(TAG, "Resetting BLE stack due to repeated scan stalls (restartCount=$scanRestartCount)")
        emitDiagnostic("warning", "Resetting BLE stack due to repeated scan stalls", mapOf(
            "restartCount" to scanRestartCount
        ))
        
        lastAdapterReset = now
        scanRestartCount = 0
        
        // Force stop and restart everything
        bleHandler.post {
            if (state == TransportState.RUNNING) {
                stopScanning("ble_reset")
                stopAdvertising()
                
                // Re-initialize scanner and advertiser
                bluetoothLeScanner = bluetoothAdapter?.bluetoothLeScanner
                leAdvertiser.attachAdvertiser(bluetoothAdapter?.bluetoothLeAdvertiser)
                
                // Restart after a brief delay
                bleHandler.postDelayed({
                    if (state == TransportState.RUNNING) {
                        startScanning("ble_reset")
                        startAdvertising("ble_reset")
                    }
                }, 1000)
            }
        }
    }
    
    // Per-peer BLE write gate. Android 13+ (API 33) rejects a second
    // writeCharacteristic on the same connection with
    // ERROR_GATT_WRITE_REQUEST_BUSY (201) while one write is still
    // outstanding — even for WRITE_TYPE_NO_RESPONSE — so the drain loop's
    // back-to-back writes used to self-collide and silently drop a
    // multi-fragment message. We allow at most one outstanding write per BLE
    // address and release it from [onWriteCompleted] (the onCharacteristicWrite
    // callback). The value is the elapsedRealtime() the write was issued, used
    // as a watchdog so a lost completion callback cannot wedge a peer forever.
    // BLE-thread only (every accessor runs under assertOnBleThread).
    private val writeInFlight = HashMap<String, Long>()

    // MARK: - TransportManager Implementation
    
    private fun emitDiagnostic(level: String, message: String, context: Map<String, Any?> = emptyMap()) {
        diagnosticEmitter?.invoke(level, message, context)
    }

    private fun reportBleAvailability(isAvailable: Boolean, reason: String) {
        if (reportedBleAvailable == isAvailable) return

        try {
            protocol.bleStatusChanged(isAvailable)
            reportedBleAvailable = isAvailable
            Log.i(TAG, "Reported BLE availability=$isAvailable (reason=$reason)")
        } catch (e: Exception) {
            if (logThrottler.shouldLog("ble_status_report_failed", intervalMs = 60_000L)) {
                Log.e(TAG, "Failed to report BLE availability=$isAvailable", e)
                emitDiagnostic(
                    "error",
                    "Failed to report BLE availability",
                    mapOf(
                        "available" to isAvailable,
                        "reason" to reason,
                        "exception" to e.javaClass.simpleName,
                        "message" to (e.message ?: "unknown"),
                    ),
                )
            }
        }
    }

    override fun isAvailable(): Boolean {
        if (bluetoothAdapter == null) {
            Log.w(TAG, "Bluetooth adapter not available")
            emitDiagnostic("error", "Bluetooth adapter not available")
            return false
        }
        
        if (!context.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH_LE)) {
            Log.w(TAG, "BLE not supported on this device")
            emitDiagnostic("error", "BLE not supported on this device")
            return false
        }
        
        return true
    }
    
    override fun start() {
        runOnBleThreadSync {
            startUnsafe()
        }
    }

    private fun startUnsafe() {
        if (state == TransportState.RUNNING) {
            throw TransportException.AlreadyRunning()
        }

        if (!isAvailable()) {
            throw TransportException.NotAvailable("BLE not available on this device")
        }

        // Lower the shutdown barrier for this new session. stopUnsafe leaves
        // it raised so late binder callbacks from the previous session keep
        // early-returning; a fresh start must explicitly re-open the gate.
        shuttingDown = false
        adapterWasOff = false
        
        // Check permissions with detailed logging
        Log.i(TAG, "Checking Bluetooth permissions (Android ${Build.VERSION.SDK_INT})...")
        emitDiagnostic("info", "Checking Bluetooth permissions", mapOf("androidVersion" to Build.VERSION.SDK_INT))
        
        if (!checkPermissions()) {
            val errorMsg = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                "Missing required Bluetooth permissions (BLUETOOTH_SCAN, BLUETOOTH_ADVERTISE, BLUETOOTH_CONNECT). " +
                "Please grant permissions in Settings > Apps > ${context.applicationInfo.loadLabel(context.packageManager)} > Permissions"
            } else {
                "Missing required Bluetooth permissions (BLUETOOTH, BLUETOOTH_ADMIN, ACCESS_FINE_LOCATION). " +
                "Please grant permissions in app settings."
            }
            Log.w(TAG, "$errorMsg")
            emitDiagnostic("error", errorMsg)
            throw TransportException.PermissionDenied(errorMsg)
        }
        
        if (bluetoothAdapter?.isEnabled != true) {
            val errorMsg = "Bluetooth is not enabled. Please enable Bluetooth in Settings."
            Log.w(TAG, "$errorMsg")
            emitDiagnostic("error", errorMsg)
            throw TransportException.InvalidState(errorMsg)
        }
        
        Log.i(TAG, "Starting BLE transport for device: $deviceId")
        emitDiagnostic("info", "Starting BLE transport", mapOf("deviceId" to deviceId))
        updateState(TransportState.STARTING)
        
        try {
            // Initialize scanner
            Log.i(TAG, "Initializing BLE scanner...")
            bluetoothLeScanner = bluetoothAdapter.bluetoothLeScanner
            if (bluetoothLeScanner == null) {
                throw TransportException.InvalidState("BLE scanner is not available")
            }
            
            // Initialize advertiser
            Log.i(TAG, "Initializing BLE advertiser...")
            val advertiser = bluetoothAdapter.bluetoothLeAdvertiser
                ?: throw TransportException.InvalidState("BLE advertiser is not available")
            leAdvertiser.attachAdvertiser(advertiser)
            
            // Setup GATT server
            Log.i(TAG, "Setting up GATT server...")
            // Keep the working central role when peripheral setup fails.
            // setupGattServer reports the failure, and advertising remains
            // deferred behind the service-ready gate.
            setupGattServer()

            transportStartAt = System.currentTimeMillis()
            meshController.markPeerActive(deviceId)
            refreshSelfMetrics()
            
            // Start advertising
            Log.i(TAG, "Starting BLE advertising...")
            startAdvertising("start")
            
            // Start scanning
            Log.i(TAG, "Starting BLE scanning...")
            startScanning("start")
            
            // Start fragment polling
            bleHandler.post(fragmentPollingRunnable)
            
            // Start routing cleanup
            bleHandler.postDelayed(routingCleanupRunnable, ROUTING_CLEANUP_INTERVAL_MS)
            
            updateState(TransportState.RUNNING)
            if (isScanning) {
                reportBleAvailability(true, "start")
            }
            
            Log.i(TAG, "BLE transport ready - scanning and advertising active")
            emitDiagnostic(
                "info",
                "BLE manager running",
                mapOf(
                    "scanning" to true,
                    "advertising" to true,
                    "mtu" to MAX_FRAGMENT_SIZE
                )
            )
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start BLE manager", e)
            emitDiagnostic(
                "error",
                "Failed to start BLE manager",
                mapOf(
                    "message" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                )
            )
            updateState(TransportState.STOPPED)
            throw TransportException.StartFailed("Failed to start BLE manager", e)
        }
    }
    
    override fun stop() {
        runOnBleThreadSync {
            stopUnsafe()
        }
    }

    // Called via runOnBleThreadSync from stop(), so this always executes on the BLE thread.
    // removeCallbacks below guarantees no further polling/drain runnables will fire,
    // making the subsequent .clear() calls safe against concurrent access.
    private fun stopUnsafe() {
        if (state != TransportState.RUNNING && state != TransportState.STARTING) {
            return
        }

        updateState(TransportState.STOPPING)

        // Raise the shutdown barrier BEFORE clearing any shared state.
        // Binder-thread GATT callbacks (`handleReceivedData`, connection
        // state changes, CCCD writes) check this flag and early-return, so
        // anything arriving after this point cannot observe half-cleared
        // maps. The flag is @Volatile, which is sufficient because every
        // reader is a callback that races with a single BLE-thread writer.
        shuttingDown = true

        // Stop fragment polling — must happen before clearing queues
        bleHandler.removeCallbacks(fragmentPollingRunnable)

        // Drop any pending backpressure re-drain and reset the ladder. This
        // instance is reused across a disable/enable cycle, so a stale rung
        // would otherwise make the next session's first stall retry slowly —
        // or, at the ceiling, not at all.
        backpressureRetry.cancel()

        // Stop routing cleanup
        bleHandler.removeCallbacks(routingCleanupRunnable)

        // Stop scanning
        stopScanning("stop")

        // Stop advertising
        stopAdvertising()

        // Disconnect all GATT clients. `disconnect`+`close` are idempotent
        // and late binder callbacks from still-draining operations are
        // absorbed by the [shuttingDown] gate above.
        connections.forEachGatt { gatt ->
            try {
                gatt.disconnect()
                gatt.close()
            } catch (e: Exception) {
                Log.e(TAG, "Error closing GATT client", e)
            }
        }
        connections.clear()
        lastSeenRssi.clear()
        pendingInbound.clear()
        outboundQueue.clear()
        // Drop any negotiated-MTU values staged by the prior session.
        // Per-disconnect paths (dropStagedPeerMtu) already handle most
        // entries, but a transport stop mid-handshake (MTU staged,
        // device id not yet resolved) leaves an orphan keyed by BLE
        // address that no disconnect path would clear. Android BLE
        // addresses are stable per device, so a subsequent
        // start+reconnect to the same address would otherwise observe
        // a stale value from the prior session before the new
        // handshake stages its own MTU.
        peerMaxPayloads.clear()
        peripheralMaxPayloads.clear()
        centralPayloadByDevice.clear()
        peripheralPayloadByDevice.clear()
        // The per-peer write gate is otherwise self-healing across a restart
        // (watchdogs fire within the gate window and the stale-check covers a
        // fast restart since elapsedRealtime is monotonic), but clear it here to
        // match the documented "drop prior-session state" intent and remove the
        // cross-session reasoning burden.
        writeInFlight.clear()
        centralClient.clearAll()
        lastSeenMeshAdvertisements.clear()
        verifiedNonMeshDevices.clear()
        unknownBootstrapAttempts.clear()
        recentAdvertisementHashes.clear()
        scanRestartCount = 0
        lastAdapterReset = 0L
        adapterWasOff = false
        transportStartAt = 0L
        lastProactiveScanRefresh = 0L
        lastForcedBleRefresh = 0L

        // Close GATT server (stops service, clears subscribed centrals, drops refs).
        peripheralGattServer?.stop()
        peripheralGattServer = null
        leAdvertiser.shutdown()

        // Reset identity refresh backoff so a subsequent start() begins fresh.
        identityRefreshRetryScheduled = false
        identityRefreshRetryDelayMs = IDENTITY_REFRESH_MIN_BACKOFF_MS
        identityRefreshAttempts = 0
        identityRefreshGivenUp = false
        cachedSignedIdentity = null

        updateState(TransportState.STOPPED)
        reportBleAvailability(false, "stop")
        // Scope the dedup to this session. One facade instance is reused
        // across disable/enable, and the module also reports
        // bleStatusChanged(false) out of band on its disable path — so a value
        // carried across the boundary can silently suppress the next session's
        // first report and leave the core holding BLE unavailable for the whole
        // of it while this transport scans normally.
        reportedBleAvailable = null

        // Leave [shuttingDown] raised until [startUnsafe] explicitly lowers
        // it. Any late binder callbacks from the previous session that
        // arrive between stop and a future start must no-op — lowering the
        // gate here would re-admit them into cleared state.

        Log.i(TAG, "BLE Manager stopped")
        emitDiagnostic("info", "BLE transport stopped")
    }
    
    override fun pause() {
        runOnBleThreadSync {
            pauseUnsafe()
        }
    }
    
    private fun pauseUnsafe() {
        // For Android background mode
        stopScanning("pause")
        bleHandler.removeCallbacks(fragmentPollingRunnable)
        bleHandler.removeCallbacks(routingCleanupRunnable)
    }
    
    override fun resume() {
        runOnBleThreadSync {
            resumeUnsafe()
        }
    }
    
    private fun resumeUnsafe() {
        // Resume from background
        if (state == TransportState.RUNNING) {
            startScanning("resume")
            bleHandler.post(fragmentPollingRunnable)
            bleHandler.postDelayed(routingCleanupRunnable, ROUTING_CLEANUP_INTERVAL_MS)
        }
    }
    
    // MARK: - Private Methods
    
    private fun updateState(newState: TransportState) {
        state = newState
        listener?.onTransportStateChanged(this, newState)
    }
    
    private fun checkPermissions(): Boolean {
        val missingPermissions = mutableListOf<String>()
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Android 12+ (API 31+) requires new Bluetooth permissions
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_SCAN")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_ADVERTISE")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_CONNECT")
            }
        } else {
            // Pre-Android 12 (API <31)
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADMIN) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("BLUETOOTH_ADMIN")
            }
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED) {
                missingPermissions.add("ACCESS_FINE_LOCATION")
            }
        }
        
        if (missingPermissions.isNotEmpty()) {
            Log.w(TAG, "Missing Bluetooth permissions: ${missingPermissions.joinToString(", ")}")
            emitDiagnostic("error", "Missing Bluetooth permissions", mapOf(
                "missingPermissions" to missingPermissions,
                "androidVersion" to Build.VERSION.SDK_INT
            ))
            return false
        }
        
        Log.d(TAG, "All Bluetooth permissions granted (Android ${Build.VERSION.SDK_INT})")
        return true
    }
    
    private fun setupGattServer(): Boolean {
        try {
            // Dispose any previous server before starting a new one.
            peripheralGattServer?.stop()

            val server = PeripheralGattServer(
                context = context,
                bleHandler = bleHandler,
                listener = gattServerListener,
                diagnosticEmitter = { level, message, ctx ->
                    emitDiagnostic(level, message, ctx)
                },
            )
            peripheralGattServer = server

            // Prime cached identity synchronously so the first GATT read from
            // a central can be served off the volatile cache without the
            // binder thread ever calling back into UniFFI. If MLS isn't up
            // yet, queue a bounded-backoff retry so we don't leave the cache
            // stuck null forever.
            if (!updateSignedIdentity()) {
                ensureIdentityRefreshScheduled()
            }

            server.start(
                serviceUuid = SERVICE_UUID,
                messageUuid = MESSAGE_CHAR_UUID,
                deviceIdUuid = DEVICE_ID_CHAR_UUID,
                identityUuid = IDENTITY_CHAR_UUID,
            )

            Log.i(TAG, "GATT server setup initiated, waiting for service registration callback...")
            emitDiagnostic("info", "GATT server setup initiated")
            return true
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while setting up GATT server", e)
            emitDiagnostic("error", "Permission denied in GATT server setup", mapOf("exception" to e.javaClass.simpleName))
            throw e
        } catch (e: Exception) {
            Log.e(TAG, "Error setting up GATT server: ${e.message}", e)
            emitDiagnostic("error", "Error setting up GATT server", mapOf(
                "exception" to e.javaClass.simpleName,
                "message" to (e.message ?: "unknown")
            ))
            return false
        }
    }
    
    /**
     * Refresh [cachedSignedIdentity] by signing the current advertisement
     * data with the identity private key. Must only be called from threads
     * that are allowed to block on UniFFI (BLE thread and advertisement
     * refresh callers); it must **never** be called from the GATT server's
     * binder callback thread, because each call potentially blocks on the
     * protocol mutex and stalls every pending GATT operation for the
     * affected central.
     *
     * Returns true if the cache was successfully refreshed. Returns false
     * if MLS is not yet initialized or signing threw — in which case the
     * caller should arrange a retry via [ensureIdentityRefreshScheduled].
     */
    private fun updateSignedIdentity(): Boolean {
        try {
            if (!protocol.isMlsInitialized()) {
                Log.d(TAG, "MLS not initialized, cannot create signed identity")
                return false
            }

            // Cache the advertised address alongside the signed identity. Both
            // are served from binder callbacks, which must never touch the
            // protocol mutex, and both need MLS initialized — so they are
            // primed together, on this thread, by the same retry ladder.
            //
            // This is what `DEVICE_ID` serves. It is deliberately not the
            // `deviceId` constructor argument: that is the app-chosen profile,
            // a local storage selector with no key behind it. Advertising it
            // let a peer claim any name, and — because the core stamps
            // `localAddress()` as `Message.sender` — also made our own control
            // frames fail the receiver's `validate_transport_sender`.
            cachedLocalAddress = protocol.localAddress()

            val publicKey = protocol.getIdentityPublicKey()
            val meshData = meshController.toAdvertisement()
            val advertisementData = meshData.encode()
            val signature = protocol.signData(advertisementData.map { it.toUByte() })

            cachedSignedIdentity = com.offlineprotocol.mesh.SignedIdentityData(
                publicKey = publicKey.map { it.toByte() }.toByteArray(),
                signature = signature.map { it.toByte() }.toByteArray(),
                advertisementData = advertisementData
            )
            identityRefreshRetryDelayMs = IDENTITY_REFRESH_MIN_BACKOFF_MS
            identityRefreshAttempts = 0
            identityRefreshGivenUp = false
            Log.d(TAG, "Updated signed identity for GATT serving")
            return true
        } catch (e: Exception) {
            Log.w(TAG, "Failed to create signed identity: ${e.message}", e)
            emitDiagnostic("warning", "Failed to create signed identity", mapOf("error" to (e.message ?: "unknown")))
            return false
        }
    }

    /**
     * Ensure the identity cache is eventually primed even when the first
     * attempt failed (typically because MLS init hasn't landed yet).
     * Schedules a single bounded-backoff retry on the BLE thread. Idempotent
     * — calling while a retry is already queued is a no-op.
     *
     * Bounded by [MAX_IDENTITY_REFRESH_ATTEMPTS]: once the budget is exhausted
     * the [identityRefreshGivenUp] latch is set, a terminal diagnostic is
     * emitted, and further calls become no-ops until [stop] clears the latch.
     * This protects against an infinite retry loop when MLS is permanently
     * broken (e.g. corrupted keychain) and the GATT-server binder thread is
     * posting refresh requests on every central read.
     *
     * Must be called from the BLE thread.
     */
    private fun ensureIdentityRefreshScheduled() {
        assertOnBleThread("ensureIdentityRefreshScheduled")
        if (identityRefreshGivenUp) return
        if (identityRefreshRetryScheduled) return
        if (cachedSignedIdentity != null) return
        if (state != TransportState.RUNNING && state != TransportState.STARTING) return
        identityRefreshRetryScheduled = true
        val delay = identityRefreshRetryDelayMs
        bleHandler.postDelayed({
            identityRefreshRetryScheduled = false
            if (state != TransportState.RUNNING && state != TransportState.STARTING) return@postDelayed
            if (cachedSignedIdentity != null) return@postDelayed
            identityRefreshAttempts++
            if (!updateSignedIdentity()) {
                if (identityRefreshAttempts >= MAX_IDENTITY_REFRESH_ATTEMPTS) {
                    identityRefreshGivenUp = true
                    Log.e(
                        TAG,
                        "Identity refresh exhausted after $identityRefreshAttempts attempts; " +
                            "GATT identity reads will fail until the transport is restarted",
                    )
                    emitDiagnostic(
                        "error",
                        "Identity refresh exhausted",
                        mapOf(
                            "attempts" to identityRefreshAttempts,
                            "maxAttempts" to MAX_IDENTITY_REFRESH_ATTEMPTS,
                        ),
                    )
                    return@postDelayed
                }
                identityRefreshRetryDelayMs = minOf(
                    IDENTITY_REFRESH_MAX_BACKOFF_MS,
                    identityRefreshRetryDelayMs * 2,
                )
                ensureIdentityRefreshScheduled()
            }
        }, delay)
    }
    
    private fun startScanning(reason: String = "manual") {
        if (isScanning) {
            if (logThrottler.shouldLog("scan_already_running")) {
                Log.d(TAG, "Scan already running (reason: $reason)")
            }
            return
        }

        // Check the adapter before touching the scanner. The framework refuses
        // startScan while the adapter is off by throwing, and the watchdog
        // restart path reaches it on every heartbeat for as long as the user
        // leaves Bluetooth off — so without this, the steady state is a fixed
        // cadence of exceptions and diagnostics. Deferring here keeps the catch
        // below for the genuine race (the adapter can still go off between this
        // check and the call).
        if (bluetoothAdapter?.isEnabled != true) {
            adapterWasOff = true
            scheduleBleRecovery()
            reportBleAvailability(false, "adapter_off")
            if (logThrottler.shouldLog("scan_adapter_off", intervalMs = 60_000L)) {
                Log.i(TAG, "Deferring scan start — BT adapter is off (reason: $reason)")
                emitDiagnostic("info", "Deferring BLE scan start — adapter off", mapOf("reason" to reason))
            }
            return
        }

        try {
            val scanner = bluetoothLeScanner
            if (scanner == null) {
                // Recoverable: the adapter reset path nulls this out when it
                // re-reads the scanner while the adapter is down, and only
                // [bleRecoveryRunnable] re-reads it afterwards. Arm before the
                // app-supplied diagnostic emitter runs.
                scheduleBleRecovery()
                if (logThrottler.shouldLog("scanner_unavailable")) {
                    Log.w(TAG, "BluetoothLeScanner unavailable; cannot start scan")
                    emitDiagnostic("error", "BLE scanner unavailable", mapOf("reason" to reason))
                }
                return
            }
            val scanSettings = ScanSettings.Builder()
                .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
                .setCallbackType(ScanSettings.CALLBACK_TYPE_ALL_MATCHES)
                .build()
            
            // Scan without service UUID filter for iOS ↔ Android interoperability
            // iOS's CoreBluetooth has known issues recognizing 128-bit service UUIDs from Android
            // advertisements, and vice versa. Scanning without filter and filtering in software
            // ensures we discover all mesh devices regardless of platform quirks.
            // We filter in handleScanResult using shouldProcessDiscoveredDevice().
            
            scanCallback = object : ScanCallback() {
                // Scan callbacks arrive on a private Binder thread. Repost
                // every result to the BLE handler so handleScanResult —
                // which can synchronously reach updateSignedIdentity() via
                // evictPeer → refreshAdvertising and which mutates
                // LeAdvertiser state that is otherwise BLE-thread only —
                // never runs off the BLE thread. Matches the threading model used by
                // the GATT server listener callbacks.
                override fun onScanResult(callbackType: Int, result: ScanResult) {
                    bleHandler.post { handleScanResult(result) }
                }

                override fun onBatchScanResults(results: List<ScanResult>) {
                    bleHandler.post { results.forEach { handleScanResult(it) } }
                }

                override fun onScanFailed(errorCode: Int) {
                    // Bind the callback identity explicitly rather than leaning
                    // on `this` resolving through the posted lambda below.
                    val self = this
                    val errorMsg = when(errorCode) {
                        SCAN_FAILED_ALREADY_STARTED -> "Scan already started"
                        SCAN_FAILED_APPLICATION_REGISTRATION_FAILED -> "Application registration failed"
                        SCAN_FAILED_INTERNAL_ERROR -> "Internal error"
                        SCAN_FAILED_FEATURE_UNSUPPORTED -> "Feature unsupported"
                        else -> "Unknown error $errorCode"
                    }
                    // Repost to the BLE thread so the state mutation and the runnable
                    // teardown match the threading contract the rest of the
                    // facade follows.
                    bleHandler.post {
                        // Ignore a failure belonging to a callback we have
                        // already replaced or torn down — the scan we would
                        // stop is a newer, possibly healthy one, and a
                        // deliberate stop/pause must not be undone. Checked
                        // ahead of the logging below so a stale failure is
                        // silent as well as inert. Same identity check
                        // LeAdvertiser.onStartFailure uses.
                        if (scanCallback !== self) return@post
                        // Route through stopScanning instead of clearing the
                        // flags inline. Reaching this callback means startScan
                        // returned normally — the refusal is asynchronous — so
                        // isScanning is still true and the framework may yet
                        // hold a scanner registration for this callback: the
                        // client entry is inserted before registration is
                        // attempted, and only stopScan removes it. stopScanning
                        // releases it and runs the one shared teardown; leaving
                        // it and re-entering startScanning with a fresh callback
                        // would strand one entry per attempt against a per-app
                        // cap of 5.
                        val isTerminal = errorCode == SCAN_FAILED_FEATURE_UNSUPPORTED
                        try {
                            stopScanning(
                                "scan_failed",
                                preserveRecoveryBackoff = !isTerminal,
                            )
                        } finally {
                            // stopScanning emits through app code after its
                            // teardown. The terminal status update or transient
                            // retry must happen even if that emitter throws.
                            if (isTerminal) {
                                // The hardware cannot do what we are asking, so
                                // retrying only burns wakeups for the process
                                // lifetime. Remove BLE from DORS instead.
                                reportBleAvailability(false, "scan_feature_unsupported")
                            } else {
                                // This is the same recovery episode. Teardown
                                // preserved the next rung, so persistent async
                                // refusals climb 10s -> 20s -> 30s rather than
                                // resetting to 10s on every callback.
                                scheduleBleRecovery()
                            }
                        }

                        // Log only after the state is coherent and any retry is
                        // armed. The diagnostic emitter crosses into app code
                        // and must not be able to strand the facade mid-failure.
                        if (logThrottler.shouldLog("scan_failed", intervalMs = 60_000L)) {
                            Log.e(TAG, "BLE scan failed: $errorMsg (code=$errorCode)")
                            emitDiagnostic("error", "BLE scan failed", mapOf(
                                "errorCode" to errorCode,
                                "errorMessage" to errorMsg
                            ))
                        }
                    }
                }
            }
            
            // Scan without filter - we'll filter in software for cross-platform compatibility
            try {
                scanner.startScan(null, scanSettings, scanCallback)
            } catch (e: IllegalStateException) {
                // Lost the race against the adapter check above: the framework
                // throws IllegalStateException("BT Adapter is not turned ON")
                // from the scanner. Swallow so the watchdog's restart path
                // cannot crash the host app, and hand the retry to
                // [bleRecoveryRunnable] — none of the scan-health timers survive
                // a failed start. Scoped to this one call so an
                // IllegalStateException raised anywhere else in this function —
                // a BLE-thread `check`, a throwing diagnostic emitter — still
                // fails loud instead of being reported as an adapter-off.
                scanCallback = null
                adapterWasOff = true
                scheduleBleRecovery()
                reportBleAvailability(false, "adapter_off_race")
                Log.i(TAG, "Skipping startScan — BT adapter not on: ${e.message}")
                emitDiagnostic("info", "Skipping startScan — BT adapter not on", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                return
            }
            isScanning = true
            // pause() deliberately cancels pending recovery. If this scan was
            // started by a later resume() while the adapter-off latch is still
            // raised, re-arm the task that repairs GATT and advertising.
            bleRecoveryScheduler.onScanStarted(adapterWasOff)
            // Deliberately no ladder reset here. Reaching this line proves only
            // that startScan *returned*; the refusal can still arrive
            // asynchronously at onScanFailed, and resetting on the synchronous
            // return would pin a stack that keeps failing that way to the
            // bottom rung forever — retry, return, reset, fail, retry. The reset
            // lives in [handleScanResult] instead, which is the first point that
            // proves a scan is actually alive.
            val now = System.currentTimeMillis()
            lastDiscoveryAt = now
            lastProactiveScanRefresh = now
            // Reset restart count on non-watchdog starts
            if (reason != "restart_watchdog") {
                scanRestartCount = 0
            }
            scheduleScanWatchdog()
            startConnectionMonitor()
            if (logThrottler.shouldLog("scan_started")) {
                Log.i(TAG, "BLE scanning started (no filter, reason: $reason)")
                emitDiagnostic("info", "BLE scanning started", mapOf(
                    "reason" to reason,
                    "filterless" to true
                ))
            }
            
            // Rehydrate previously connected devices to avoid waiting for advertisements
            rehydratePreviouslyConnectedDevices()
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while starting scan", e)
            emitDiagnostic("error", "Permission denied while starting scan", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            throw e
        }
    }
    
    /**
     * Attempts to reconnect to previously known devices without waiting for advertisements.
     * This speeds up rediscovery after app restart or Bluetooth toggle.
     */
    private fun rehydratePreviouslyConnectedDevices() {
        try {
            val bondedDevices = bluetoothAdapter?.bondedDevices ?: return
            for (device in bondedDevices) {
                val address = device.address
                // Only attempt if we previously had this device in our registry
                if (connections.hasDeviceForAddress(address) && connections.getGatt(address) == null) {
                    if (logThrottler.shouldLog("rehydrate_$address", intervalMs = 30_000)) {
                        Log.d(TAG, "Rehydrating connection to bonded device: $address")
                    }
                    connectToDevice(device)
                }
            }
        } catch (e: SecurityException) {
            Log.w(TAG, "Cannot access bonded devices for rehydration", e)
        }
    }
    
    private fun stopScanning(
        reason: String = "manual",
        preserveRecoveryBackoff: Boolean = false,
    ) {
        // Cancel ahead of the isScanning guard: a pending recovery exists
        // precisely when isScanning is false, and every caller that gets here —
        // stop, pause, restart, the refresh paths — means "do not be scanning
        // right now". Async scan failure is the sole exception: it still
        // removes the pending callback, but preserves the next backoff rung for
        // the retry that follows this teardown.
        cancelBleRecovery(resetBackoff = !preserveRecoveryBackoff)
        if (!isScanning) return

        val stopFailure: Exception? = try {
            scanCallback?.let { bluetoothLeScanner?.stopScan(it) }
            null
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while stopping scan", e)
            e
        } catch (e: IllegalStateException) {
            // The adapter can transition off between the guard above and the
            // call, and the framework then throws IllegalStateException("BT
            // Adapter is not turned ON").
            Log.i(TAG, "Skipping stopScan — BT adapter not on: ${e.message}")
            e
        }

        // Teardown runs on every outcome, which is why it sits outside the try.
        // For the adapter-off throw the framework-side scan is already dead and
        // local state simply has to follow. For a SecurityException it is not:
        // the call was refused, so the scan may well still be registered, and
        // dropping scanCallback below discards the only reference that could
        // ever stop it. That is accepted rather than equivalent — Android kills
        // the process on a runtime permission revocation, so the stranded
        // registration dies with it — and the alternative is worse in both
        // cases: a lingering isScanning short-circuits the next startScanning
        // at its guard, and the watchdog and connection monitor keep firing
        // against a scanner that is gone.
        scanCallback = null
        isScanning = false
        cancelScanWatchdog()
        cancelConnectionMonitor()
        lastDiscoveryAt = 0L
        discoveryLogTimestamps.clear()

        when (stopFailure) {
            is SecurityException -> emitDiagnostic(
                "error",
                "Permission denied while stopping scan",
                mapOf(
                    "exception" to stopFailure.javaClass.simpleName,
                    "message" to (stopFailure.message ?: "unknown"),
                ),
            )
            is IllegalStateException -> emitDiagnostic(
                "info",
                "Skipping stopScan — BT adapter not on",
                mapOf(
                    "exception" to stopFailure.javaClass.simpleName,
                    "message" to (stopFailure.message ?: "unknown"),
                ),
            )
        }

        if (logThrottler.shouldLog("scan_stopped")) {
            Log.i(TAG, "Stopped scanning (reason: $reason)")
        }
        emitDiagnostic("info", "Stopped BLE scanning", mapOf("reason" to reason))
    }
    
    private fun restartScanning(reason: String) {
        stopScanning("restart_$reason")
        startScanning("restart_$reason")
    }
    
    private fun scheduleScanWatchdog() {
        cancelScanWatchdog()
        bleHandler.postDelayed(scanWatchdogRunnable, SCAN_WATCHDOG_HEARTBEAT_MS)
    }
    
    private fun cancelScanWatchdog() {
        bleHandler.removeCallbacks(scanWatchdogRunnable)
    }

    private fun scheduleBleRecovery() {
        bleRecoveryScheduler.schedule()
    }

    private fun cancelBleRecovery(resetBackoff: Boolean = true) {
        bleRecoveryScheduler.cancel(resetBackoff)
    }
    
    /**
     * Starts the connection monitor that periodically attempts to reconnect to discovered devices.
     * This mirrors iOS's startConnectionMonitor mechanism for more reliable discovery.
     */
    private fun startConnectionMonitor() {
        cancelConnectionMonitor()
        
        connectionMonitorRunnable = object : Runnable {
            override fun run() {
                if (state != TransportState.RUNNING) {
                    return
                }
                
                val now = System.currentTimeMillis()
                
                // Check for discovered devices that aren't connected
                for ((address, observation) in lastSeenMeshAdvertisements) {
                    // Skip if already connected
                    if (connections.getGatt(address) != null) {
                        continue
                    }
                    
                    // Skip if we've hit connection cap
                    if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
                        break
                    }
                    
                    // Skip if observation is too old
                    if (now - observation.timestamp > MESH_OBSERVATION_TTL_MS) {
                        continue
                    }
                    
                    // Skip if RSSI too weak
                    val rssi = observation.rssi ?: continue
                    if (rssi < MINIMUM_RSSI_TO_CONNECT) {
                        continue
                    }
                    
                    // Rate limit attempts to this device
                    val lastAttempt = deviceConnectionAttempts[address]
                    if (lastAttempt != null && now - lastAttempt < MIN_RECONNECT_INTERVAL_MS) {
                        continue
                    }
                    
                    // Try to connect
                    try {
                        val device = bluetoothAdapter?.getRemoteDevice(address) ?: continue
                        recordConnectionAttempt(address, now)
                        connectToDevice(device)
                    } catch (e: Exception) {
                        Log.w(TAG, "Connection monitor: failed to get remote device $address", e)
                    }
                }
                
                // Also check for pending fragments that need device ID resolution
                val pendingAddresses = pendingInbound.pendingAddresses()
                for (address in pendingAddresses) {
                    if (connections.deviceIdForAddress(address) != null) {
                        continue
                    }
                    
                    val lastAttempt = centralClient.lastResolutionAttempt(address)
                    if (lastAttempt != null && now - lastAttempt < MIN_RECONNECT_INTERVAL_MS) {
                        continue
                    }

                    centralClient.markResolutionAttempt(address, now)
                    try {
                        val device = bluetoothAdapter?.getRemoteDevice(address)
                        if (device != null && connections.getGatt(address) == null) {
                            connectToDevice(device)
                        }
                    } catch (e: Exception) {
                        Log.w(TAG, "Connection monitor: failed to resolve device ID for $address", e)
                    }
                }
                
                bleHandler.postDelayed(this, CONNECTION_MONITOR_INTERVAL_MS)
            }
        }
        
        bleHandler.postDelayed(connectionMonitorRunnable!!, CONNECTION_MONITOR_INTERVAL_MS)
    }
    
    private fun cancelConnectionMonitor() {
        connectionMonitorRunnable?.let { bleHandler.removeCallbacks(it) }
        connectionMonitorRunnable = null
    }
    
    // Advertising is owned by LeAdvertiser. These thin wrappers preserve
    // the existing call sites in this file (refreshAdvertising / stopAdvertising
    // are invoked from many places) without threading the delegate object
    // through every caller.

    private fun startAdvertising(reason: String = "manual") {
        leAdvertiser.start(reason)
    }

    private fun stopAdvertising() {
        leAdvertiser.stop()
    }

    private fun refreshAdvertising(reason: String) {
        leAdvertiser.refresh(reason)
    }

    private fun buildAdvertiseData(): AdvertiseData {
        val meshData = meshController.toAdvertisement()
        lastMeshAdvertisement = meshData
        
        // Android has strict 31-byte advertisement limit
        // Include only service UUID, mesh metadata will be exchanged via GATT after connection
        // This matches iOS behavior which also cannot include service data
        return AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            // Don't include service data - it often exceeds Android's 31-byte limit
            // Mesh metadata will be read via GATT characteristics after connection
            .build()
    }
    
    /**
     * Builds the scan response data for BLE advertising.
     * 
     * iOS's CoreBluetooth actively queries for scan responses during BLE scanning.
     * Including the service UUID in the scan response makes Android devices more
     * reliably visible to iOS devices, which have known issues recognizing 128-bit
     * service UUIDs from Android's main advertisement packet format.
     */
    private fun buildScanResponse(): AdvertiseData {
        return AdvertiseData.Builder()
            .setIncludeDeviceName(false)
            .addServiceUuid(ParcelUuid(SERVICE_UUID))
            .build()
    }
    
    private fun handleScanResult(result: ScanResult) {
        // Scan results are posted here from a binder thread, so one produced
        // just before stopScan lands *behind* [stopUnsafe] on the main queue.
        // Such a straggler must not be treated as evidence of a live scan: it
        // would re-report BLE as available for a transport that is already
        // stopped, re-stamp lastDiscoveryAt, reset the backoff rung
        // [onScanFailed] deliberately preserved, and walk the connection path
        // against maps stopUnsafe has just cleared.
        if (shuttingDown || !isScanning) return

        val device = result.device
        val rssi = result.rssi
        val address = device.address
        val now = System.currentTimeMillis()
        lastDiscoveryAt = now
        // A result in hand is the one proof the scan is genuinely live, which is
        // what ends a recovery episode and resets its backoff ladder. Anchored
        // here rather than on a successful startScan return because that return
        // does not rule out an asynchronous onScanFailed a moment later. A
        // device with no peers in range simply holds at the cap, which costs
        // nothing: every deliberate stop resets the ladder via stopScanning.
        if (!adapterWasOff) {
            bleRecoveryScheduler.resetBackoff()
            reportBleAvailability(true, "scan_result")
        }

        // Duplicate advertisement detection - avoid processing identical advertisements
        // This improves performance in dense networks
        val advertHash = computeAdvertisementHash(result)
        val cached = recentAdvertisementHashes[address]
        if (cached != null && cached.hash == advertHash && now - cached.timestamp < 1000L) {
            return // Skip duplicate advertisement
        }
        recentAdvertisementHashes[address] = AdvertisementCacheEntry(advertHash, now)
        
        // Prune old advertisement cache entries periodically
        if (recentAdvertisementHashes.size > 100) {
            val cutoff = now - 30_000L
            val iterator = recentAdvertisementHashes.entries.iterator()
            while (iterator.hasNext()) {
                if (iterator.next().value.timestamp < cutoff) {
                    iterator.remove()
                }
            }
        }
        
        // Adaptive scanning: track discoveries for density estimation
        recordDiscoveryForDensity(now)
        
        // Software-based filtering for iOS ↔ Android interoperability
        // Since we scan without a service UUID filter, we filter here instead.
        val scanRecord = result.scanRecord
        val isConnectable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            result.isConnectable
        } else {
            true // Assume connectable on older Android
        }
        
        if (!shouldProcessDiscoveredDevice(address, scanRecord, rssi, isConnectable, now)) {
            return
        }
        
        // Adaptive scanning: early RSSI filtering in dense networks
        if (shouldFilterByRssi(rssi)) {
            if (logThrottler.shouldLog("adaptive_rssi_filter", intervalMs = 10000)) {
                Log.d(TAG, "Adaptive: filtering weak signal (${rssi}dBm) in dense network ($estimatedVisiblePeerCount peers)")
            }
            return
        }
        
        // Adaptive scanning: probabilistic filtering in very dense networks
        if (shouldProbabilisticallySkip(address)) {
            return // Silently skip to reduce log spam in dense networks
        }
        
        // Extract service information for logging
        val serviceUuids = scanRecord?.serviceUuids
        val serviceData = scanRecord?.getServiceData(ParcelUuid(SERVICE_UUID))
        
        val lastLog = discoveryLogTimestamps[address]
        if (lastLog == null || now - lastLog > 30000) {
            discoveryLogTimestamps[address] = now
            val hasServiceUuid = serviceUuids?.any { it.uuid == SERVICE_UUID } == true
            val hasServiceData = serviceData != null
            Log.d(TAG, "Discovered device $address RSSI=$rssi (density: $estimatedVisiblePeerCount, hasServiceUuid: $hasServiceUuid, hasServiceData: $hasServiceData)")
            emitDiagnostic(
                "info",
                "Discovered BLE device",
                mapOf(
                    "address" to address,
                    "rssi" to rssi,
                    "connectable" to isConnectable,
                    "visiblePeers" to estimatedVisiblePeerCount,
                    "hasServiceUuid" to hasServiceUuid,
                    "hasServiceData" to hasServiceData,
                    "serviceUuids" to (serviceUuids?.map { it.uuid.toString() } ?: emptyList())
                )
            )
        }
        lastSeenRssi[address] = rssi.toShort()
        val meshMetadata = MeshAdvertisementData.decode(serviceData)
        meshMetadata?.let {
            lastSeenMeshAdvertisements[address] = MeshObservation(it, rssi, now)
        }
        meshController.observeAdvertisement(meshMetadata, rssi)
        pruneMeshObservations(now)

        // When there's no metadata (iOS/Android advertising without service data),
        // still try to connect - metadata will be exchanged via GATT after connection
        val decision = if (meshMetadata == null) {
            // No metadata in advertisement - allow basic connection to exchange info via GATT
            MeshController.MeshDecision(
                intent = ConnectionIntent.INTRA_CLUSTER,
                reason = "no_metadata_in_advert",
                evictPeerId = null
            )
        } else {
            meshController.shouldInitiateOutbound(meshMetadata, rssi)
        }
        
        if (decision.intent == ConnectionIntent.REJECTED) {
            if (logThrottler.shouldLog("mesh_skip_$address", intervalMs = 15000)) {
                Log.v(TAG, "Skipping connection to $address due to ${decision.reason}")
            }
            return
        }
        
        // Adaptive scanning: rate limit connection attempts
        // Skip throttling for first-time discoveries with strong signals for faster connection
        val isFirstDiscovery = !lastSeenMeshAdvertisements.containsKey(address) && connections.getGatt(address) == null
        val hasStrongSignal = rssi >= -70
        
        if (!isFirstDiscovery || !hasStrongSignal) {
            if (shouldThrottleConnection(address, now)) {
                if (logThrottler.shouldLog("adaptive_throttle_$address", intervalMs = 30000)) {
                    Log.d(TAG, "Adaptive: throttling connection to $address")
                }
                return
            }
        } else if (isFirstDiscovery && hasStrongSignal) {
            Log.d(TAG, "Fast-tracking first discovery with strong signal: $address RSSI=$rssi")
            emitDiagnostic("info", "Fast-tracking first discovery", mapOf(
                "address" to address,
                "rssi" to rssi
            ))
        }

        if (!meshController.connectionBudgetAvailable() && decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, decision.reason)
        }

        val desiredRole = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER -> MeshRole.MEMBER
            ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }

        if (!meshController.connectionBudgetAvailable()) {
            if (logThrottler.shouldLog("mesh_budget_exhausted", intervalMs = 5000)) {
                Log.d(TAG, "Connection budget exhausted, skipping $address")
            }
            return
        }
        
        // Record the connection attempt for rate limiting
        recordConnectionAttempt(address, now)

        if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
            if (logThrottler.shouldLog("mesh_conn_cap", intervalMs = 10000)) {
                Log.d(TAG, "Reached max simultaneous connections, skipping $address")
            }
            return
        }

        if (connections.getGatt(address) == null) {
            connections.setPendingRole(address, desiredRole)
            connectToDevice(device)
        } else if (logThrottler.shouldLog("discovery_existing_$address", intervalMs = 30000)) {
            Log.v(TAG, "Device $address already connected/connecting")
        }

        maybeHandleRebalance("scan")
        
        // Check if we should proactively refresh the scan
        maybeProactivelyRefreshScan(now)
    }
    
    /**
     * Determines if a discovered device should be processed.
     * Implements smart filtering since we scan without a service UUID filter
     * (required for iOS ↔ Android interoperability).
     *
     * Accepts:
     * - Devices advertising our service UUID
     * - Devices with our service data
     * - Previously discovered mesh devices
     * - Previously verified peer/device mappings
     * - Strictly rate-limited bootstrap attempts for unknown connectable devices
     */
    private fun shouldProcessDiscoveredDevice(
        address: String,
        scanRecord: android.bluetooth.le.ScanRecord?,
        rssi: Int,
        isConnectable: Boolean,
        now: Long
    ): Boolean {
        // 0. Skip devices previously verified as non-mesh via GATT
        val nonMeshTimestamp = verifiedNonMeshDevices[address]
        if (nonMeshTimestamp != null) {
            if (now - nonMeshTimestamp < NON_MESH_CACHE_TTL_MS) {
                logDiscoveryRejection(address, "non_mesh_cache", now, mapOf("ageMs" to (now - nonMeshTimestamp)))
                return false
            }
            // Entry expired, remove it and allow re-evaluation
            verifiedNonMeshDevices.remove(address)
        }
        
        // 1. Check if device is advertising our service UUID
        val serviceUuids = scanRecord?.serviceUuids
        if (serviceUuids != null) {
            for (uuid in serviceUuids) {
                // Check both full UUID and short form for cross-platform compatibility
                if (uuid.uuid == SERVICE_UUID || uuid.toString().uppercase() == SERVICE_UUID.toString().uppercase()) {
                    if (logThrottler.shouldLog("service_uuid_match_$address", intervalMs = 30_000)) {
                        Log.d(TAG, "Device $address matches service UUID: ${uuid.uuid}")
                    }
                    return true
                }
            }
        }
        
        // Also check service UUIDs in scan record AD structures (for iOS compatibility)
        // iOS sometimes advertises service UUIDs in a format Android's API doesn't parse correctly.
        // Restrict this fallback to 128-bit Service UUID AD fields only.
        val scanRecordBytes = scanRecord?.bytes
        if (scanRecordBytes != null && containsServiceUuidInAdStructures(scanRecordBytes)) {
            if (logThrottler.shouldLog("service_uuid_bytes_match_$address", intervalMs = 30_000)) {
                Log.d(TAG, "Device $address matches service UUID in scan record AD structures")
            }
            return true
        }
        
        // 2. Check for our service data
        val serviceData = scanRecord?.getServiceData(ParcelUuid(SERVICE_UUID))
        if (serviceData != null) {
            return true
        }
        
        // 3. Check if this is a previously discovered mesh device
        if (lastSeenMeshAdvertisements.containsKey(address)) {
            return true
        }
        
        // 4. Check if we already have a device ID mapping for this device
        if (connections.deviceIdForAddress(address) != null) {
            return true
        }
        
        // 5. Check if we have an active GATT connection to this device
        if (connections.getGatt(address) != null) {
            return true
        }

        // 6. Controlled bootstrap for unknown connectable devices.
        // Missing advertisement fields are treated as unknown (not invalid), but we keep
        // strict safeguards to avoid probing arbitrary peripherals.
        if (shouldAllowUnknownBootstrap(address, scanRecord != null, rssi, isConnectable, now)) {
            if (logThrottler.shouldLog("bootstrap_allow_$address", intervalMs = 30_000L)) {
                Log.d(TAG, "Allowing provisional bootstrap for $address (rssi=$rssi, scanRecord=${scanRecord != null})")
                emitDiagnostic("debug", "Allowing provisional bootstrap candidate", mapOf(
                    "address" to address,
                    "rssi" to rssi,
                    "scanRecordPresent" to (scanRecord != null),
                    "connectable" to isConnectable
                ))
            }
            return true
        }
        
        // Filter out all other devices (not our mesh network)
        logDiscoveryRejection(address, "unknown_candidate_blocked", now, mapOf(
            "rssi" to rssi,
            "connectable" to isConnectable,
            "scanRecordPresent" to (scanRecord != null)
        ))
        return false
    }

    private fun containsServiceUuidInAdStructures(scanRecordBytes: ByteArray): Boolean {
        var offset = 0
        while (offset < scanRecordBytes.size) {
            val length = scanRecordBytes[offset].toInt() and 0xFF
            if (length == 0) break

            val nextStructureOffset = offset + length + 1
            if (nextStructureOffset > scanRecordBytes.size) {
                return false
            }
            if (length < 2) {
                offset = nextStructureOffset
                continue
            }

            val adType = scanRecordBytes[offset + 1].toInt() and 0xFF
            if (adType == AD_TYPE_INCOMPLETE_128_BIT_SERVICE_UUIDS || adType == AD_TYPE_COMPLETE_128_BIT_SERVICE_UUIDS) {
                val dataStart = offset + 2
                val dataLength = length - 1
                val uuidCount = dataLength / UUID_128_BIT_LENGTH_BYTES

                for (uuidIndex in 0 until uuidCount) {
                    val uuidOffset = dataStart + (uuidIndex * UUID_128_BIT_LENGTH_BYTES)
                    var matches = true
                    for (byteIndex in 0 until UUID_128_BIT_LENGTH_BYTES) {
                        if (scanRecordBytes[uuidOffset + byteIndex] != SERVICE_UUID_LE_BYTES[byteIndex]) {
                            matches = false
                            break
                        }
                    }
                    if (matches) return true
                }
            }

            offset = nextStructureOffset
        }
        return false
    }

    private fun shouldAllowUnknownBootstrap(
        address: String,
        hasScanRecord: Boolean,
        rssi: Int,
        isConnectable: Boolean,
        now: Long
    ): Boolean {
        val lastAttempt = unknownBootstrapAttempts[address]
        val oneMinuteAgo = now - 60_000L
        val recentBootstrapAttempts = unknownBootstrapAttempts.values.count { it >= oneMinuteAgo }

        val recentConnectionAttempts = synchronized(globalConnectionAttempts) {
            globalConnectionAttempts.count { it >= oneMinuteAgo }
        }
        val shouldAllow = BleDiscoveryBootstrapPolicy.shouldAllowCandidate(
            isConnectable = isConnectable,
            currentConnectionCount = currentConnectionCount(),
            maxConnectionsPerDevice = MAX_CONNECTIONS_PER_DEVICE,
            estimatedVisiblePeerCount = estimatedVisiblePeerCount,
            densePeerThreshold = ADAPTIVE_HIGH_DENSITY_THRESHOLD,
            rssi = rssi,
            hasScanRecord = hasScanRecord,
            minRssiWithScanRecord = UNKNOWN_BOOTSTRAP_MIN_RSSI,
            minRssiWithoutScanRecord = UNKNOWN_BOOTSTRAP_MIN_RSSI_NO_SCAN_RECORD,
            lastAttemptAt = lastAttempt,
            now = now,
            perDeviceCooldownMs = UNKNOWN_BOOTSTRAP_RATE_LIMIT_MS,
            recentBootstrapAttempts = recentBootstrapAttempts,
            maxBootstrapAttemptsPerMinute = MAX_UNKNOWN_BOOTSTRAP_ATTEMPTS_PER_MINUTE,
            recentConnectionAttempts = recentConnectionAttempts,
            maxConnectionAttemptsPerMinute = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE
        )
        if (!shouldAllow) return false

        unknownBootstrapAttempts[address] = now
        return true
    }

    private fun logDiscoveryRejection(
        address: String,
        reason: String,
        now: Long,
        details: Map<String, Any?> = emptyMap()
    ) {
        if (!logThrottler.shouldLog("reject_${reason}_$address", intervalMs = 30_000L, nowMs = now)) {
            return
        }
        Log.v(TAG, "Skipping discovered device $address ($reason)")
        emitDiagnostic("debug", "Skipping discovered BLE device", details + mapOf(
            "address" to address,
            "reason" to reason
        ))
    }
    
    /**
     * Proactively refreshes the scan periodically to ensure we don't miss devices
     * due to BLE stack issues or cached state.
     */
    private fun maybeProactivelyRefreshScan(now: Long) {
        if (now - lastProactiveScanRefresh >= PROACTIVE_SCAN_REFRESH_MS) {
            lastProactiveScanRefresh = now
            if (logThrottler.shouldLog("proactive_scan_refresh", intervalMs = PROACTIVE_SCAN_REFRESH_MS)) {
                Log.d(TAG, "Proactively refreshing BLE scan")
                emitDiagnostic("info", "Proactive scan refresh")
            }
            restartScanning("proactive_refresh")
        }
        
        // Forced complete BLE refresh - more aggressive than proactive refresh
        // This helps recover from edge cases where the BLE stack becomes stuck
        val lastForced = if (lastForcedBleRefresh == 0L) transportStartAt else lastForcedBleRefresh
        if (now - lastForced >= FORCED_BLE_REFRESH_MS) {
            lastForcedBleRefresh = now
            if (logThrottler.shouldLog("forced_ble_refresh", intervalMs = FORCED_BLE_REFRESH_MS)) {
                Log.i(TAG, "Performing forced BLE refresh for reliability")
                emitDiagnostic("info", "Forced BLE refresh for reliability", mapOf(
                    "connectedPeers" to connections.connectionCount(),
                    "discoveredPeers" to connections.discoveredPeerCount()
                ))
            }
            // Stop and restart both scanning and advertising
            stopScanning("forced_refresh")
            refreshAdvertising("forced_refresh")
            bleHandler.postDelayed({
                if (state == TransportState.RUNNING) {
                    startScanning("forced_refresh")
                }
            }, 500)
        }
    }
    
    /**
     * Computes a hash of the advertisement data for duplicate detection.
     * Uses device address, RSSI bucket, and key advertisement data.
     */
    private fun computeAdvertisementHash(result: ScanResult): Int {
        var hash = result.device.address.hashCode()
        // Use RSSI buckets of 5 dBm to avoid hash changes from minor signal fluctuations
        hash = 31 * hash + (result.rssi / 5)
        
        val scanRecord = result.scanRecord
        if (scanRecord != null) {
            // Include service UUIDs
            scanRecord.serviceUuids?.forEach { uuid ->
                hash = 31 * hash + uuid.hashCode()
            }
            
            // Include service data
            val serviceData = scanRecord.getServiceData(ParcelUuid(SERVICE_UUID))
            if (serviceData != null) {
                hash = 31 * hash + serviceData.contentHashCode()
            }
        }
        
        return hash
    }
    
    /** Lock for atomic connection count check and connect operations */
    private val connectionLock = Any()
    
    private fun connectToDevice(device: BluetoothDevice) {
        try {
            // Atomic check-and-connect to prevent race conditions
            synchronized(connectionLock) {
                // Check RSSI threshold - don't connect to devices with weak signals
                val rssi = lastSeenRssi[device.address]?.toInt() ?: -60
                if (rssi < MINIMUM_RSSI_TO_CONNECT) {
                    if (logThrottler.shouldLog("rssi_skip_${device.address}", intervalMs = 10000)) {
                        Log.d(TAG, "Skipping connection to ${device.address} due to weak RSSI ($rssi < $MINIMUM_RSSI_TO_CONNECT)")
                        emitDiagnostic("debug", "Skipping BLE connect due to weak RSSI", mapOf(
                            "address" to device.address,
                            "rssi" to rssi,
                            "threshold" to MINIMUM_RSSI_TO_CONNECT
                        ))
                    }
                    connections.consumePendingRole(device.address)
                    return
                }
                
                if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
                    if (logThrottler.shouldLog("mesh_conn_cap", intervalMs = 10000)) {
                        Log.d(TAG, "Connection cap reached, not connecting to ${device.address}")
                    }
                    connections.consumePendingRole(device.address)
                    return
                }
                
                // Double-check we don't already have a connection to this device
                if (connections.getGatt(device.address) != null) {
                    if (logThrottler.shouldLog("already_connecting_${device.address}", intervalMs = 5000)) {
                        Log.d(TAG, "Already have GATT client for ${device.address}")
                    }
                    return
                }
                
                val gatt = device.connectGatt(context, false, centralClient.callback, BluetoothDevice.TRANSPORT_LE)
                if (gatt == null) {
                    // BluetoothDevice.connectGatt is declared @Nullable in the
                    // Android framework — Kotlin treats it as a platform type,
                    // but MeshConnectionRegistry.registerGatt takes a non-null
                    // BluetoothGatt, so a null return trips the compiler-
                    // inserted Intrinsics.checkNotNullParameter and crashes
                    // with NullPointerException. connectGatt returns null when
                    // the adapter has just turned off, the underlying hardware
                    // handle is stale, or the device unbonded between scan and
                    // connect — the same adapter-race conditions that motivate
                    // the SecurityException handler below. Release the pending
                    // role reserved for this address (mirroring the RSSI and
                    // connection-cap skip paths above) so state does not hang
                    // half-set-up, and emit a diagnostic so callers can see
                    // how often the race fires in the field.
                    Log.i(TAG, "connectGatt returned null for ${device.address} — adapter unavailable")
                    emitDiagnostic("info", "Skipping connectGatt — adapter unavailable", mapOf("address" to device.address))
                    connections.consumePendingRole(device.address)
                    return
                }
                connections.registerGatt(device.address, gatt)
            }
            
            Log.i(TAG, "Connecting to device: ${device.address}")
            emitDiagnostic("info", "Connecting to BLE device", mapOf("address" to device.address))
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied while connecting to device", e)
            emitDiagnostic("error", "Permission denied while connecting to device", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            connections.consumePendingRole(device.address)
        }
    }

    private fun currentConnectionCount(): Int = connections.connectionCount()

    /**
     * The subscribed peripheral-link address that maps to [deviceId] by identity,
     * or null if the peer is not notify-subscribed. Wraps [resolveSubscribedAddress]
     * over the live GATT-server subscription set and the connection registry so the
     * MTU floor and the notify egress resolve notify-reachability identically — see
     * [resolveSubscribedAddress] for why that shared predicate is load-bearing.
     * BLE-thread only (reads [connections] and the GATT-server snapshot).
     */
    private fun subscribedNotifyAddressFor(deviceId: String): String? {
        val server = peripheralGattServer ?: return null
        return resolveSubscribedAddress(deviceId, server.subscribedAddresses()) {
            connections.deviceIdForAddress(it)
        }
    }

    /**
     * Recomputes and flushes the effective per-peer ATT payload into the Rust
     * transport keyed by [deviceId]. The core stores ONE MTU per peer but a
     * peer can be reachable over two links with different MTUs — the central
     * link WE opened ([peerMaxPayloads]) and the peripheral/NOTIFY link the
     * peer opened to our GATT server ([peripheralMaxPayloads]). A single
     * fragment stream must fit BOTH, so we flush the **minimum** of whatever
     * is currently known. Without this, a message sized for the (larger)
     * central link but egressed as a notify overflows the peripheral link and
     * is truncated/dropped on air — the offline 1:1 Welcome-delivery stall.
     *
     * The two links can use DIFFERENT BLE addresses for the same peer (iOS uses
     * distinct connection handles per direction) and Rust is keyed by device id,
     * so the min MUST be taken per DEVICE, not per address: a single call sees
     * only one link's [address], so it promotes that address's staged value into
     * the matching per-device direction slot ([centralPayloadByDevice] /
     * [peripheralPayloadByDevice]) and mins across those. This accumulates both
     * directions across the two links' separate flushes, so a late renegotiation
     * on one link can no longer drop the bound the other imposed.
     *
     * Called whenever either link's MTU is (re)negotiated, when the device id
     * resolves ([CentralGattClient.Host.onDeviceIdResolved]), and when a link
     * tears down (to restore the surviving link's MTU). If neither direction has
     * a value the Rust entry is cleared so the fragmenter reverts to its
     * 185-byte floor rather than retaining a dead link's value — unless the peer
     * is still notify-subscribed, in which case the peripheral term is floored to
     * the 185-byte fragment cap (via [computeEffectivePayload]) so a notify-
     * egressed message (an MLS Welcome) is never sized for the central link when
     * the notify link's own MTU was never observed. Idempotent and
     * BLE-thread only. The per-address staged values are NOT removed on flush
     * (they are inputs to the min); link-teardown paths clear the dropped
     * direction's per-device slot explicitly.
     */
    private fun flushPeerMtu(address: String, deviceId: String) {
        assertOnBleThread("flushPeerMtu")
        // Promote this link's staged (per-address) payload into the per-DEVICE
        // direction slot. On asymmetric-address peers each flush sees only its own
        // link's address, so both directions accumulate across the two links'
        // separate flushes; on a symmetric (single-address) peer both promote
        // together. Promotion only sets — never clears — so a flush via one link
        // cannot wipe the bound the other imposed.
        peerMaxPayloads[address]?.let { centralPayloadByDevice[deviceId] = it }
        peripheralMaxPayloads[address]?.let { peripheralPayloadByDevice[deviceId] = it }

        val central = centralPayloadByDevice[deviceId]
        // The notify link's MTU is only observable via the GATT-server
        // onMtuChanged, which is unreliable for the server role and often never
        // fires (an iOS central negotiates a smaller MTU on the link it opens to
        // us, or none). Without a peripheral value the min() would collapse to the
        // central payload and a multi-fragment notify — an MLS Welcome — would
        // overflow the notify link and be silently truncated on air. So when the
        // peer is notify-subscribed but no peripheral payload is on file,
        // [computeEffectivePayload] floors the peripheral term to the 185-byte
        // fragment cap until a real value arrives. Notify-reachability is resolved
        // by [subscribedNotifyAddressFor] — the SAME device-scoped resolution the
        // notify egress uses — so the floor is applied for exactly the peers the
        // notify path can reach.
        val peripheralStaged = peripheralPayloadByDevice[deviceId]
        val notifySubscribed = subscribedNotifyAddressFor(deviceId) != null
        val effective = computeEffectivePayload(
            central = central,
            peripheralStaged = peripheralStaged,
            notifySubscribed = notifySubscribed,
            floor = MAX_FRAGMENT_SIZE,
        )
        if (effective == null) {
            // Both directions gone — drop the Rust entry so the fragmenter falls
            // back to the floor instead of keeping a stale per-peer size, and the
            // now-empty per-device slots.
            centralPayloadByDevice.remove(deviceId)
            peripheralPayloadByDevice.remove(deviceId)
            try {
                protocol.bleClearPeerMtu(deviceId)
            } catch (e: Exception) {
                Log.w(TAG, "bleClearPeerMtu failed for $deviceId", e)
            }
            return
        }
        // The notify floor took effect AND it caps a central link that could carry
        // more: the per-peer MTU is ONE value used for BOTH directions, so flooring
        // for the unobserved notify link also throttles central-write egress to the
        // 185-byte cap until a real peripheral MTU is observed (then
        // min(central, peripheral) relaxes it). The trade is correct — silent notify
        // truncation is the alternative — and self-healing, but surface it so a
        // convergence/throughput investigation can see WHY a higher-MTU central link
        // is fragmenting at 185. (Honoring per-direction fragment sizes, which would
        // remove the trade, is a deeper fragmenter change tracked separately.)
        if (peripheralStaged == null && notifySubscribed && central != null && central > MAX_FRAGMENT_SIZE) {
            emitDiagnostic(
                "info",
                "BLE per-peer MTU floored to notify cap; central egress throttled until peripheral MTU observed",
                mapOf(
                    "deviceId" to deviceId,
                    "centralPayload" to central,
                    "floor" to MAX_FRAGMENT_SIZE,
                ),
            )
        }
        // A link that negotiated BELOW the fragment floor is a real offline-
        // failure case: Rust rejects a sub-floor MTU and reverts to the 185-byte
        // floor (see `set_peer_mtu`), which is LARGER than this link can carry, so
        // a multi-fragment notify can still overflow. Rare on modern stacks (both
        // platforms request the BLE-5 max) but it is exactly the legacy/quirky-peer
        // population, so surface it to Metro instead of failing silently. (Honoring
        // a sub-floor notify fragment size is a deeper fragmenter change tracked
        // separately.)
        if (effective < MAX_FRAGMENT_SIZE) {
            emitDiagnostic(
                "warning",
                "BLE link negotiated below fragment floor; notify may overflow",
                mapOf(
                    "deviceId" to deviceId,
                    "effectivePayload" to effective,
                    "floor" to MAX_FRAGMENT_SIZE,
                ),
            )
        }
        try {
            protocol.bleSetPeerMtu(deviceId, effective.toUInt())
            emitDiagnostic(
                "info",
                "BLE per-peer MTU flushed to Rust",
                mapOf(
                    "address" to address,
                    "deviceId" to deviceId,
                    "centralPayload" to (central ?: -1),
                    "peripheralPayload" to (peripheralStaged ?: -1),
                    "effectivePayload" to effective,
                ),
            )
        } catch (e: Exception) {
            // Leave the staged entries in place on failure so the next
            // flush opportunity (re-entry via either link's negotiation or
            // onDeviceIdResolved) can retry without losing the value.
            Log.w(TAG, "bleSetPeerMtu failed for $deviceId", e)
            emitDiagnostic(
                "warning",
                "bleSetPeerMtu failed",
                mapOf("deviceId" to deviceId, "exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
        }
    }

    /**
     * Drops the facade-side staged negotiated-MTU entry for a BLE
     * address. The Rust-side `peer_mtus` entry is owned by the
     * central-role link's lifecycle — it is populated by
     * `CentralGattClient.onMtuChanged → flushPeerMtu` and cleared by
     * `protocol.blePeerLost` via `on_peer_lost`. Facade teardown
     * paths must never touch it directly: a peripheral-role
     * disconnect, eviction, or give-up for a peer that still has an
     * alive central-role link would otherwise demote that link from
     * its negotiated BLE 5 MTU back to the 185-byte floor for the
     * rest of the link's life. See the comment block in
     * [handleCentralDisconnectedOnBleThread] for the bug this invariant
     * exists to prevent.
     *
     * [address] may be null on paths where we only know the device id; the
     * per-device direction slots keyed by [deviceId] are still cleared.
     * BLE-thread only.
     */
    private fun dropStagedPeerMtu(address: String?, deviceId: String?) {
        assertOnBleThread("dropStagedPeerMtu")
        if (address != null) {
            peerMaxPayloads.remove(address)
            peripheralMaxPayloads.remove(address)
        }
        // Also clear the per-device direction slots [flushPeerMtu] promotes into,
        // or a stale value would survive this address's teardown and skew the min
        // on the peer's next link. Callers that drop the whole peer (blePeerLost)
        // pass the device id so both directions are cleared.
        if (deviceId != null) {
            centralPayloadByDevice.remove(deviceId)
            peripheralPayloadByDevice.remove(deviceId)
        }
    }

    private fun refreshSelfMetrics() {
        val rssiValues = lastSeenRssi.values.map { it.toInt() }
        val averageRssi = if (rssiValues.isEmpty()) null else rssiValues.average().roundToInt()
        val signalQuality = averageRssi?.let { rssi ->
            (((rssi + 100).coerceIn(-100, -20) + 100) / 80.0 * 100).roundToInt().coerceIn(0, 100)
        }
        // Both total-count reads are AtomicInteger snapshots and therefore
        // safe from any thread even though the underlying per-peer buffers
        // are BLE-thread only. refreshSelfMetrics is itself called from
        // main so this is belt-and-suspenders, but the AtomicInteger is what
        // lets the diagnostic assembly path stay lock-free.
        val pendingCount = pendingInbound.totalCount()
        val outboundPending = outboundQueue.totalCount()
        val totalPending = pendingCount + outboundPending
        val stability = 1.0 - min(1.0, pendingCount / 10.0)
        val batteryPercent = currentBatteryPercent()
        val loadPercent = ((totalPending.coerceAtMost(LOAD_SATURATION_COUNT) * 100) / LOAD_SATURATION_COUNT).coerceIn(0, 100)
        val uptimeSeconds = if (transportStartAt == 0L) null else ((System.currentTimeMillis() - transportStartAt) / 1000).coerceAtLeast(0)

        meshController.updateSelfMetrics(
            MeshController.PeerMetrics(
                rssi = averageRssi,
                batteryPercent = batteryPercent,
                signalQuality = signalQuality,
                stability = stability,
                uptimeSeconds = uptimeSeconds?.toLong(),
                loadPercent = loadPercent
            )
        )
        meshController.markPeerActive(deviceId)
        maybeHandleRebalance("self_metrics")
    }

    private fun currentBatteryPercent(): Int? {
        return try {
            val manager = context.getSystemService(Context.BATTERY_SERVICE) as? BatteryManager
            val capacity = manager?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: return null
            capacity.takeIf { it in 0..100 }
        } catch (_: Exception) {
            null
        }
    }

    private fun evictPeer(peerId: String, reason: String) {
        // Run the entire eviction body on the BLE thread as one atomic step.
        // The previous shape posted only `outboundQueue.removeAll` to the BLE thread and
        // ran the connection bookkeeping inline; that left a window where
        // `drainAndSendFragments` (which runs on the BLE thread) could observe the
        // already-removed GATT entry, fall through the `hasConnection == false`
        // branch, and enqueue fresh fragments to a peer that was about to have
        // its outbound queue cleared. Those fragments would orphan in the
        // queue until the per-peer cap or 30s expiry pruned them.
        //
        // Since the queue + linkReady + outboundQueue contracts are all
        // BLE-thread-only anyway, doing the whole sequence on the BLE thread is the
        // simplest way to make eviction atomic against the drain pump. If
        // we're already on the BLE thread this runs inline; otherwise we block the
        // caller's thread until main has processed the eviction. Eviction is
        // not on the hot path so the latch hop is acceptable.
        runOnBleThreadSync {
            val address = connections.addressForDevice(peerId)
            if (address == null) {
                if (logThrottler.shouldLog("mesh_evict_missing_$peerId")) {
                    Log.w(TAG, "Cannot evict $peerId: no known address")
                }
                return@runOnBleThreadSync
            }

            if (logThrottler.shouldLog("mesh_evict_$peerId", intervalMs = 5000)) {
                Log.i(TAG, "Evicting peer $peerId to reclaim capacity (reason=$reason)")
            }

            connections.getGatt(address)?.let { gatt ->
                try {
                    gatt.disconnect()
                    gatt.close()
                } catch (e: Exception) {
                    Log.w(TAG, "Error while evicting $peerId", e)
                }
            }

            connections.removeGatt(address)
            centralClient.forgetLink(address)
            connections.removeIdentifiersForDevice(peerId)
            connections.removeConnectionRole(peerId)
            lastSeenRssi.remove(address)
            pendingInbound.removeAll(address)
            outboundQueue.removeAll(peerId)
            // Facade-only staged drop; the Rust-side MTU entry is cleared
            // by `protocol.blePeerLost` below via `on_peer_lost`.
            dropStagedPeerMtu(address, peerId)
            meshController.registerDisconnection(peerId)
            refreshSelfMetrics()

            // Clean up routes through this neighbor
            protocol.removeNeighborRoutes(peerId)

            try {
                protocol.blePeerLost(peerId)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to notify protocol of peer eviction", e)
            }

            refreshAdvertising("evict_$reason")
            maybeHandleRebalance("evict")
        }
    }
    
    /**
     * Called by the Rust transport callback when new outgoing fragments are available.
     * This is the primary send path, replacing the 100ms polling loop.
     * Posts to bleHandler to ensure all BLE operations run on the BLE thread.
     */
    fun onFragmentsAvailable() {
        bleHandler.post { drainAndSendFragments() }
    }

    /**
     * Drains the Rust fragment queue and sends each fragment over BLE.
     * Stops when the queue is empty, all target peers are flow-controlled,
     * or [MAX_DRAIN_ITERATIONS_PER_CALL] is hit. In the cap-hit case the
     * remaining work is rescheduled via [bleHandler] so we yield the main
     * thread between batches; without that, a Rust-side fragment burst can
     * hold the BLE thread long enough to trigger ANR.
     *
     * Called from onFragmentsAvailable() and from the fallback polling timer.
     */
    private fun drainAndSendFragments() {
        assertOnBleThread("drainAndSendFragments")
        if (state != TransportState.RUNNING) return

        var hitIterationCap = false
        var hitBackpressure = false
        // Whether anything actually reached the BLE stack this pass. Drives
        // [backpressureRetry].reset() — a peer that is moving fragments again
        // must get the fast ladder back, or one bad stretch would leave it on
        // the polling floor for the rest of the session.
        var sentAny = false
        try {
            val flushed = outboundQueue.flush(::sendFragmentData)
            // Accepted sends only — a queue that shrank because entries hit
            // their TTL delivered nothing, and must not read as progress.
            if (flushed.sent > 0) {
                sentAny = true
            }
            if (flushed.hasUnsent) {
                hitBackpressure = true
                if (logThrottler.shouldLog("drain_flush_stalled", intervalMs = 5000)) {
                    val recipientCount = outboundQueue.recipientCount()
                    Log.w(
                        TAG,
                        "drainAndSendFragments: outbound flush left $recipientCount " +
                            "recipient(s) with unsent fragments",
                    )
                    emitDiagnostic(
                        "warning",
                        "Outbound drain flush stalled",
                        mapOf(
                            "recipientCount" to recipientCount,
                            "pending" to outboundQueue.totalCount(),
                        ),
                    )
                }
            }

            var consecutiveSkips = 0
            val maxConsecutiveSkips = 5
            val reconnectAttempted = mutableSetOf<String>()
            var iterations = 0

            while (true) {
                if (iterations >= MAX_DRAIN_ITERATIONS_PER_CALL) {
                    hitIterationCap = true
                    break
                }
                iterations++

                val fragment = try {
                    protocol.bleGetNextFragment()
                } catch (e: Exception) {
                    Log.e(TAG, "Error calling bleGetNextFragment(): ${e.message}", e)
                    return
                } ?: break

                val recipientId = fragment.recipientId
                val data = fragment.data.map { it.toByte() }.toByteArray()

                val address = resolveTargetAddress(recipientId)
                val hasConnection = address?.let { connections.getGatt(it) } != null
                if (!hasConnection) {
                    outboundQueue.enqueue(recipientId, data)
                    // Proactively attempt reconnection if we know the address (once per peer per drain)
                    if (address != null && reconnectAttempted.add(address)) {
                        bluetoothAdapter?.let { adapter ->
                            try {
                                val device = adapter.getRemoteDevice(address)
                                connectToDevice(device)
                            } catch (e: Exception) {
                                Log.e(TAG, "Error attempting reconnection for $recipientId during fragment drain", e)
                            }
                        }
                    }
                    consecutiveSkips++
                    if (consecutiveSkips >= maxConsecutiveSkips) {
                        break
                    }
                    continue
                }

                // Maintain FIFO ordering: if this recipient already has
                // fragments waiting, enqueue instead of sending directly.
                if (outboundQueue.enqueueIfBlocked(recipientId, data)) {
                    // Backpressure against the write gate: once this peer's
                    // queue is backed up, stop pulling more fragments out of
                    // the Rust core (bleGetNextFragment is a destructive pop)
                    // into the bounded per-peer queue. The write gate paces
                    // sends to ~1 fragment / WRITE_GATE_WATCHDOG_MS when the
                    // completion callback is absent, which is far slower than
                    // this loop can pull; without this stop the loop spins the
                    // whole backlog into the queue, overflows maxPerPeer, and
                    // OutboundFragmentQueue.enqueue discards the in-flight
                    // message. Leaving the backlog in Rust keeps delivery
                    // lossless — the scheduled re-drain below resumes once
                    // flush() has drained the queue back down.
                    if (outboundQueue.isBackedUp(recipientId)) {
                        hitBackpressure = true
                        break
                    }
                    continue
                }

                consecutiveSkips = 0

                if (!sendFragmentData(recipientId, data)) {
                    outboundQueue.enqueue(recipientId, data)
                    hitBackpressure = true
                    break
                }
                sentAny = true
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error in drainAndSendFragments", e)
        }

        if (state != TransportState.RUNNING) return

        // Anything that actually went out means the link is alive, so the next
        // stall deserves the fast ladder again. Reset before the re-arm below
        // so a pass that both sent and stalled starts from the floor.
        if (sentAny) {
            backpressureRetry.reset()
        }

        if (hitIterationCap) {
            // Not backpressure — we simply had more than one tick's worth of
            // work. Yield the thread and resume immediately; no ladder.
            bleHandler.post { drainAndSendFragments() }
        } else if (hitBackpressure && outboundQueue.totalCount() > 0) {
            if (!backpressureRetry.schedule() &&
                logThrottler.shouldLog("backpressure_ceiling", intervalMs = 5000)
            ) {
                val recipients = outboundQueue.recipientIds()
                Log.w(
                    TAG,
                    "drainAndSendFragments: backpressure retry ceiling reached " +
                        "(${MAX_BACKPRESSURE_RETRY_ATTEMPTS} attempts), leaving " +
                        "${outboundQueue.totalCount()} fragment(s) to the polling " +
                        "floor for $recipients",
                )
                emitDiagnostic(
                    "warning",
                    "Backpressure retry ceiling reached",
                    mapOf(
                        "attempts" to MAX_BACKPRESSURE_RETRY_ATTEMPTS,
                        "pending" to outboundQueue.totalCount(),
                        "recipients" to recipients,
                    ),
                )
            }
        }
    }

    /**
     * Fast-path release of the per-peer BLE write gate when the stack actually
     * fires onCharacteristicWrite for the outstanding write, draining the next
     * fragment immediately. This callback is unreliable for
     * WRITE_TYPE_NO_RESPONSE (often never fires), so it is only the fast path —
     * the gate is also released by the self-paced fallback scheduled in
     * sendFragmentData. Without the gate the drain loop issues writeCharacteristic
     * calls back-to-back and Android 13+ rejects the concurrent ones with
     * ERROR_GATT_WRITE_REQUEST_BUSY (201), silently dropping fragments.
     */
    fun onWriteCompleted(address: String) {
        assertOnBleThread("onWriteCompleted")
        // Address-keyed, not per-write: a stale onCharacteristicWrite for an
        // earlier write that fires after the watchdog/stale-check already advanced
        // the gate to a newer write on the same address will clear the NEWER
        // write's gate. Android's write callback carries no per-op token, so
        // completion cannot be correlated to a specific write. The worst case is a
        // single ERROR_GATT_WRITE_REQUEST_BUSY (201) on that newer write, which
        // self-heals via the caller's re-enqueue + BACKPRESSURE_RETRY — no data loss.
        writeInFlight.remove(address)
        if (state == TransportState.RUNNING) {
            // Pace the next completion-driven send by one BLE connection
            // interval instead of draining immediately. onNotificationSent /
            // onCharacteristicWrite signal that OUR controller accepted the
            // op, not that the peer has drained it — firing the next notify
            // instantly out-runs a slower central (iOS) and drops fragments
            // mid-burst, so a large multi-fragment Welcome never reassembles.
            // The brief spacing lets the peer keep up; small messages are
            // unaffected in practice.
            bleHandler.postDelayed({
                if (state == TransportState.RUNNING) {
                    drainAndSendFragments()
                }
            }, INTER_FRAGMENT_PACING_MS)
        }
    }

    private fun pollAndSendFragments() {
        assertOnBleThread("pollAndSendFragments")
        try {
            // The old logic would return early if there were unsent fragments, preventing new fragments
            // from being polled. This caused messages to get stuck when connections weren't ready.
            val flushed = outboundQueue.flush(::sendFragmentData)
            val hasUnsentFragments = flushed.hasUnsent
            // The polling floor is where a peer that burned the whole
            // backpressure ladder recovers. Clearing the ladder here is what
            // lets it get the fast retry back for its next stall; without it
            // the ceiling would be permanent for the rest of the session.
            // Gated on accepted sends, not on the queue shrinking: a stalled
            // peer sheds expired fragments every TTL window, and treating that
            // as recovery would hand the ladder back to the one peer the
            // ceiling exists to hold down.
            if (flushed.sent > 0) {
                backpressureRetry.reset()
            }

            // Still poll for new fragments even if there are unsent pending ones
            // This prevents deadlock where old fragments block new ones
            // Poll for next fragment from protocol
            val fragment = try {
                protocol.bleGetNextFragment()
            } catch (e: Exception) {
                Log.e(TAG, "Error calling bleGetNextFragment(): ${e.message}", e)
                emitDiagnostic("error", "Error calling bleGetNextFragment", mapOf(
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
                return
            }

            if (fragment == null) {
                // No fragment available - this is normal most of the time
                // But log if we have unsent fragments to help diagnose connection issues
                if (hasUnsentFragments && logThrottler.shouldLog("unsent_fragments_no_new", intervalMs = 5000)) {
                    val recipientCount = outboundQueue.recipientCount()
                    Log.w(TAG, "Have $recipientCount recipients with unsent fragments, but no new fragments to poll")
                    emitDiagnostic("warning", "Unsent fragments blocking", mapOf(
                        "recipientCount" to recipientCount,
                        "recipients" to outboundQueue.recipientIds()
                    ))
                } else if (logThrottler.shouldLog("no_fragments", intervalMs = 10000)) {
                    Log.d(TAG, "No fragments available from protocol")
                }
                return
            }

            Log.i(TAG, "GOT FRAGMENT for recipient: ${fragment.recipientId}, size: ${fragment.data.size}")
            emitDiagnostic("debug", "Polling got fragment", mapOf(
                "recipientId" to fragment.recipientId,
                "fragmentSize" to fragment.data.size
            ))

            val recipientId = fragment.recipientId
            val data = fragment.data.map { it.toByte() }.toByteArray()

            // Maintain FIFO ordering: if this recipient already has
            // fragments waiting, enqueue instead of sending directly.
            if (outboundQueue.enqueueIfBlocked(recipientId, data)) {
                return
            }

            val sendResult = sendFragmentData(recipientId, data)
            Log.d(TAG, "Fragment send result for $recipientId: $sendResult")

            if (!sendResult) {
                Log.w(TAG, "Failed to send fragment immediately, queuing for retry")
                outboundQueue.enqueue(recipientId, data)
            } else {
                Log.d(TAG, "Fragment sent successfully to $recipientId")
                emitDiagnostic("debug", "Fragment sent successfully", mapOf("recipientId" to recipientId))
                backpressureRetry.reset()
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error polling/sending fragments", e)
            emitDiagnostic("error", "Error sending BLE fragment", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }

    private fun resolveTargetAddress(recipientId: String): String? {
        if (recipientId == deviceId) {
            return null
        }
        connections.addressForDevice(recipientId)?.let { return it }
        connections.connectionRoleEntries()
            .sortedBy { entry -> if (entry.value == MeshRole.BRIDGE) 0 else 1 }
            .firstOrNull()
            ?.key
            ?.let { return connections.addressForDevice(it) }
        return null
    }

    /**
     * Reply to a peer that connected to OUR GATT server (we are its peripheral)
     * by notifying the message characteristic over the connection it opened and
     * subscribed to — the reliable egress for asymmetric mesh links, used in
     * preference to a reverse central writeCharacteristic that may vanish on a
     * half-open link. Shares the per-peer [writeInFlight] gate with the central
     * write path so notifications stay serialised (one outstanding per peer), but
     * paces on the INDICATE-aware [NOTIFY_GATE_WATCHDOG_MS] fallback — the fast
     * path is onNotificationSent firing on the indication confirmation.
     * BLE-thread only.
     */
    private fun sendViaNotify(recipientId: String, address: String, data: ByteArray): Boolean {
        assertOnBleThread("sendViaNotify")
        val server = peripheralGattServer ?: return false

        // Serialise to one outstanding indication per peer (same gate as the
        // central write path) so we don't out-run the stack's one-op-at-a-time
        // limit. The stale threshold is the INDICATE-aware watchdog: a deferred
        // fragment keeps waiting until onNotificationSent releases the gate or
        // the (longer) fallback elapses, rather than racing the confirmation.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val inFlightSince = writeInFlight[address]
            if (inFlightSince != null) {
                if (android.os.SystemClock.elapsedRealtime() - inFlightSince < NOTIFY_GATE_WATCHDOG_MS) {
                    if (logThrottler.shouldLog("notify_gated_$recipientId", intervalMs = 5000)) {
                        Log.d(TAG, "Deferring notify to $recipientId: prior notify still in flight")
                    }
                    return false
                }
                writeInFlight.remove(address)
            }
        }

        val device = try {
            bluetoothAdapter?.getRemoteDevice(address)
        } catch (e: IllegalArgumentException) {
            Log.w(TAG, "Cannot resolve device for notify to $recipientId ($address)", e)
            null
        } ?: return false

        if (!server.notifyFragment(device, data)) {
            if (logThrottler.shouldLog("notify_failed_$recipientId", intervalMs = 2000)) {
                Log.w(TAG, "Failed to notify BLE fragment for $recipientId")
                emitDiagnostic("warning", "Failed to notify BLE fragment", mapOf("recipientId" to recipientId))
            }
            return false
        }

        meshController.markPeerActive(recipientId)
        meshController.markPeerActive(deviceId)

        // Hold the per-peer gate. The fast path is onNotificationSent firing on
        // the INDICATE confirmation (real delivery) -> onWriteCompleted, which
        // releases the gate and paces the next fragment by one connection
        // interval. This watchdog only re-drives if that callback is lost; it is
        // longer than the central path's (see [NOTIFY_GATE_WATCHDOG_MS]) so it
        // does not pre-empt the confirmation round-trip and churn busy-rejects.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val stamp = android.os.SystemClock.elapsedRealtime()
            writeInFlight[address] = stamp
            bleHandler.postDelayed({
                if (writeInFlight[address] == stamp) {
                    writeInFlight.remove(address)
                    if (state == TransportState.RUNNING) {
                        drainAndSendFragments()
                    }
                }
            }, NOTIFY_GATE_WATCHDOG_MS)
        }
        return true
    }

    private fun sendFragmentData(recipientId: String, data: ByteArray): Boolean {
        // Every call site (drainAndSendFragments, pollAndSendFragments, the
        // OutboundFragmentQueue.flush callback) runs on the BLE thread already, but
        // the function calls into MeshController and issues
        // gatt.writeCharacteristic — which the Android BLE stack serialises
        // one op at a time per client. A future caller that forgets this
        // contract would interleave writes, so pin it to the runtime check
        // that the rest of the facade's BLE-thread invariants already use.
        assertOnBleThread("sendFragmentData")
        // Find GATT client for recipient
        val address = resolveTargetAddress(recipientId)
        val gatt = address?.let { connections.getGatt(it) }

        // Asymmetric-link reply: if this peer reached US as a central on our GATT
        // server and subscribed to the message characteristic, reply over THAT
        // connection (the one it opened and is actively waiting on) via a
        // peripheral notify, instead of a reverse central writeCharacteristic.
        // A central NO_RESPONSE write returns SUCCESS into a reverse link that may
        // be absent or half-open, and the bytes are silently dropped on air (no
        // status 201, no error) — so the central path "succeeds" forever while the
        // peer, never getting our reply, retransmits and eventually gives up. This
        // is the dominant offline failure for a peer that connected to us.
        //
        // Resolve the notify egress by PEER IDENTITY, not just `address`: the
        // address we hold for the peer (the central link WE opened to it,
        // addressForDevice) can differ from the address it subscribed under (the
        // link IT opened to our GATT server) — iOS uses distinct handles per
        // direction. So if the resolved address isn't the subscribed one, fall back
        // to any subscribed central that maps back to this recipient. Without this
        // the notify path silently never engages for the very peers it exists for.
        val notifyAddress = peripheralGattServer?.let { server ->
            if (address != null && server.isSubscribed(address)) {
                address
            } else {
                // Resolve by identity through the shared device-scoped predicate —
                // the same one the MTU floor uses, so floor and egress never desync.
                subscribedNotifyAddressFor(recipientId)
            }
        }
        if (notifyAddress != null) {
            if (logThrottler.shouldLog("reply_via_notify_$recipientId", intervalMs = 5000)) {
                Log.i(TAG, "Replying to $recipientId via peripheral notify (asymmetric link) addr=$notifyAddress")
                emitDiagnostic("info", "BLE reply via peripheral notify", mapOf(
                    "recipientId" to recipientId,
                    "notifyAddress" to notifyAddress,
                    "resolvedAddress" to (address ?: "null"),
                    "matchedByIdentity" to (notifyAddress != address).toString(),
                    "fragmentSize" to data.size,
                ))
            }
            return sendViaNotify(recipientId, notifyAddress, data)
        }

        // Until the remote has ack'd our CCCD write, the return path is not
        // verified and the BLE stack may still be executing the setup ops
        // (deviceId read → identity read → writeDescriptor). Issuing a write
        // now risks either silent loss (on stacks that drop the op) or
        // stalling the chain. Enqueue and let onDescriptorWrite trigger the
        // drain.
        if (address != null && !centralClient.isLinkReady(address)) {
            if (logThrottler.shouldLog("link_not_ready_$recipientId", intervalMs = 5000)) {
                Log.d(TAG, "Link to $recipientId not yet ready (CCCD unacked), deferring write")
            }
            return false
        }

        if (gatt == null) {
            //  Proactively try to connect if we don't have a connection
            // This helps resolve cases where fragments are queued but connection isn't established
            if (logThrottler.shouldLog("missing_gatt_$recipientId", intervalMs = 5000)) {
                Log.w(TAG, "No connected device for recipient: $recipientId - attempting to find and connect")
                emitDiagnostic("warning", "No connected device for BLE fragment - attempting connection", mapOf("recipientId" to recipientId))
            }
            
            // Try to find the device and connect
            if (address != null) {
                // We know the address but don't have a connection - try to reconnect
                bluetoothAdapter?.let { adapter ->
                    try {
                        val device = adapter.getRemoteDevice(address)
                        connectToDevice(device)
                    } catch (e: Exception) {
                        Log.e(TAG, "Error attempting reconnection for $recipientId", e)
                    }
                }
            } else {
                // We don't even know the address - this is a more serious issue
                // The device ID might not be resolved yet or route might not exist
                Log.w(TAG, "Cannot resolve address for recipient: $recipientId")
            }
            return false
        }

        // Serialise to a single outstanding write per peer (see [writeInFlight]).
        // On API 33+ a concurrent writeCharacteristic returns
        // ERROR_GATT_WRITE_REQUEST_BUSY (201) and the fragment is lost, so
        // defer (return false → drainAndSendFragments re-enqueues) until the
        // prior write's onCharacteristicWrite releases the gate. A gate older
        // than the watchdog is cleared so a lost callback cannot wedge us.
        // `address` is non-null here: `gatt` is `address?.let { ... }` and we
        // returned above when `gatt == null`, so Kotlin smart-casts it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val inFlightSince = writeInFlight[address]
            if (inFlightSince != null) {
                if (android.os.SystemClock.elapsedRealtime() - inFlightSince < WRITE_GATE_WATCHDOG_MS) {
                    if (logThrottler.shouldLog("write_gated_$recipientId", intervalMs = 5000)) {
                        Log.d(TAG, "Deferring write to $recipientId: prior write still in flight")
                    }
                    return false
                }
                writeInFlight.remove(address)
            }
        }

        //  Validate connection state before attempting to send
        if (gatt.device.bondState == BluetoothDevice.BOND_NONE) {
            // Device is not bonded - this might be okay for BLE, but log it
            if (logThrottler.shouldLog("unbonded_device_$recipientId", intervalMs = 10000)) {
                Log.d(TAG, "Device $recipientId is not bonded (this may be normal for BLE)")
            }
        }
        
        val service = gatt.getService(SERVICE_UUID)
        val characteristic = service?.getCharacteristic(MESSAGE_CHAR_UUID)
        
        if (service == null || characteristic == null) {
            if (logThrottler.shouldLog("missing_char_$recipientId")) {
                Log.w(TAG, "Message characteristic not found for recipient: $recipientId")
                emitDiagnostic("warning", "Message characteristic missing", mapOf("recipientId" to recipientId))
            }
            return false
        }

        characteristic.writeType = BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE

        // On API 33+ we use the value-parameter overload, which returns a
        // BluetoothStatusCodes result. On older APIs we have to set the shared
        // characteristic value field and call the legacy overload — its
        // Boolean return *is* load-bearing: it reports false when the internal
        // TX queue is full, another GATT op is in flight, or the characteristic
        // is unwritable. Treating every pre-Tiramisu call as success silently
        // drops fragments, which is exactly the class of bug the rest of this
        // code is trying to defend against.
        val writeOk = try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val result = gatt.writeCharacteristic(characteristic, data, BluetoothGattCharacteristic.WRITE_TYPE_NO_RESPONSE)
                if (result != BluetoothStatusCodes.SUCCESS) {
                    Log.w(TAG, "Write characteristic returned non-success status: $result for recipient: $recipientId")
                    emitDiagnostic("warning", "BLE write returned non-success status", mapOf(
                        "recipientId" to recipientId,
                        "status" to result.toString()
                    ))
                    false
                } else {
                    true
                }
            } else {
                @Suppress("DEPRECATION")
                characteristic.value = data
                @Suppress("DEPRECATION")
                val queued = gatt.writeCharacteristic(characteristic)
                if (!queued) {
                    emitDiagnostic("warning", "BLE writeCharacteristic returned false", mapOf(
                        "recipientId" to recipientId,
                    ))
                }
                queued
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error writing characteristic to $recipientId", e)
            emitDiagnostic("error", "Error writing BLE fragment", mapOf(
                "recipientId" to recipientId,
                "exception" to e.javaClass.simpleName,
                "message" to (e.message ?: "unknown")
            ))
            false
        }

        if (!writeOk) {
            if (logThrottler.shouldLog("write_failed_$recipientId", intervalMs = 2000)) {
                Log.w(TAG, "Failed to write BLE fragment for $recipientId")
                emitDiagnostic("warning", "Failed to write BLE fragment", mapOf("recipientId" to recipientId))
            }
            return false
        }
        
        // Write was initiated successfully (actual completion is asynchronous for WRITE_TYPE_NO_RESPONSE)
        meshController.markPeerActive(recipientId)
        meshController.markPeerActive(deviceId)

        // Hold the per-peer write gate. onCharacteristicWrite releases it when
        // it fires — but for WRITE_TYPE_NO_RESPONSE that completion callback is
        // stack-dependent and on many devices never fires at all. Without a
        // fallback that strands every fragment after the first: the gate set
        // here would only ever clear on the next *attempted* send, and nothing
        // reliably re-attempts, so a multi-fragment message delivers fragment 1
        // and then stalls. So self-pace — schedule a re-drain after the
        // watchdog window that releases the gate and pumps the next fragment,
        // making delivery independent of the callback. Gated on API 33+ to
        // match the serialise check above; older stacks queue writes fine.
        // `address` is non-null here: `gatt` is `address?.let { ... }` and we
        // returned above when `gatt == null`, so Kotlin smart-casts it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val stamp = android.os.SystemClock.elapsedRealtime()
            writeInFlight[address] = stamp
            bleHandler.postDelayed({
                // Only act if THIS write is still the outstanding one.
                // onCharacteristicWrite or a newer write may have already moved
                // the gate on; clearing it then would let the next write race an
                // in-flight one into status 201.
                if (writeInFlight[address] == stamp) {
                    writeInFlight.remove(address)
                    if (state == TransportState.RUNNING) {
                        drainAndSendFragments()
                    }
                }
            }, WRITE_GATE_WATCHDOG_MS)
        }
        return true
    }
    
    private fun handleReceivedData(data: ByteArray, address: String) {
        try {
            if (shuttingDown) return
            assertOnBleThread("handleReceivedData")
            // Decide queue-vs-direct: if the device ID isn't resolved yet, or
            // if earlier fragments for this address are still buffered waiting
            // on that resolution, we must buffer this fragment too to keep
            // FIFO order per peer. Both this helper and the
            // [CentralGattClient] drain path run on the BLE thread (the
            // binder callbacks above post here via bleHandler), so a
            // single-threaded decision is race-free — no lock is required.
            val resolvedSender = connections.deviceIdForAddress(address)
            val hasPendingForAddress = pendingInbound.hasPending(address)
            // Inline the queue-vs-direct condition (rather than a `val queued`)
            // so Kotlin can smart-cast `resolvedSender` to non-null after this
            // guard returns: smart-casting tracks the inline `== null` disjunct
            // here, but NOT a null check stored in a separate Boolean `val`.
            if (resolvedSender == null || hasPendingForAddress) {
                pendingInbound.enqueue(address, data)
                if (resolvedSender == null) {
                    if (logThrottler.shouldLog("queue_pending_$address")) {
                        Log.d(TAG, "Queued fragment while awaiting device ID for $address")
                        emitDiagnostic(
                            "info",
                            "Queued BLE fragment pending device ID",
                            mapOf("address" to address, "length" to data.size)
                        )
                    }

                    // Proactively attempt to resolve the device ID by initiating a client connection.
                    // We're already on a caller-controlled thread (usually a binder GATT callback);
                    // [connectToDevice] is safe to invoke from any thread, so there is no reason to
                    // pay an extra handler hop here — the historical `mainHandler.post` widened the
                    // queue-vs-drain race window above.
                    bluetoothAdapter?.let { adapter ->
                        try {
                            val device = adapter.getRemoteDevice(address)
                            val hasGattClient = connections.getGatt(device.address) != null
                            val mappedId = connections.deviceIdForAddress(device.address)
                            val now = System.currentTimeMillis()
                            val lastAttempt = centralClient.lastResolutionAttempt(address) ?: 0L
                            val shouldAttempt = now - lastAttempt > InboundFragmentBuffer.DEFAULT_TIMEOUT_MS
                            if ((!hasGattClient || mappedId.isNullOrEmpty()) && shouldAttempt) {
                                centralClient.markResolutionAttempt(address, now)
                                if (logThrottler.shouldLog("resolve_device_$address", intervalMs = 5000)) {
                                    Log.d(TAG, "Attempting to resolve device ID for $address via client connection")
                                    emitDiagnostic(
                                        "debug",
                                        "Resolving BLE sender device ID",
                                        mapOf("address" to address, "hasGattClient" to hasGattClient, "knownId" to (mappedId != null))
                                    )
                                }
                                connectToDevice(device)
                            }
                        } catch (e: IllegalArgumentException) {
                            if (logThrottler.shouldLog("resolve_device_error_$address", intervalMs = 10000)) {
                                Log.w(TAG, "Failed to obtain remote device for address $address", e)
                                emitDiagnostic(
                                    "warning",
                                    "Failed to resolve BLE device for pending fragment",
                                    mapOf("address" to address, "message" to (e.message ?: "unknown"))
                                )
                            }
                        }
                    }
                }

                // Clean up stale pending fragments across all peers while
                // we're on the BLE thread; this is the same eviction sweep
                // the routing cleanup ticker runs on its own cadence.
                pendingInbound.evictExpired()
                return
            }

            // Device ID is already resolved and no earlier bytes are waiting
            // — process directly. Kotlin smart-casts `resolvedSender` to
            // non-null here: the `resolvedSender == null` disjunct in the guard
            // above returns, so reaching this point proves it is non-null.
            val resolvedSenderId: String = resolvedSender

            lastSeenRssi[address]?.toInt()?.let { observedRssi ->
                meshController.updatePeerMetrics(
                    resolvedSenderId,
                    MeshController.PeerMetrics(rssi = observedRssi)
                )
            }
            meshController.markPeerActive(resolvedSenderId)
            meshController.markPeerActive(deviceId)

            // Convert to UByte list
            val bytes = data.map { it.toUByte() }

            // Pass to protocol
            Log.i(TAG, "RECEIVED FRAGMENT from $resolvedSenderId, size: ${data.size}")
            emitDiagnostic("info", "Fragment received from BLE", mapOf(
                "senderId" to resolvedSenderId,
                "fragmentSize" to data.size
            ))

            try {
                protocol.bleFragmentReceived(resolvedSenderId, bytes)
                Log.i(TAG, "Fragment processed successfully for sender: $resolvedSenderId")

                // Drain all completed messages (a fragment may complete multiple messages)
                var completedMessage = protocol.receiveMessage()
                if (completedMessage == null) {
                    Log.d(TAG, "Fragment processed, waiting for more fragments to complete message")
                }
                while (completedMessage != null) {
                    Log.i(TAG, "COMPLETE MESSAGE ASSEMBLED FROM FRAGMENTS!")
                    Log.i(TAG, "Received message: $completedMessage")
                    emitDiagnostic("info", "Complete message assembled from fragments", mapOf(
                        "senderId" to resolvedSenderId,
                        "messageContent" to completedMessage
                    ))
                    learnRouteFromMessage(completedMessage, resolvedSenderId, address)
                    completedMessage = protocol.receiveMessage()
                }
            } catch (e: Exception) {
                Log.e(TAG, "Error processing fragment from $resolvedSenderId: ${e.message}", e)
                emitDiagnostic("error", "Error processing received fragment", mapOf(
                    "senderId" to resolvedSenderId,
                    "fragmentSize" to data.size,
                    "error" to (e.message ?: "unknown"),
                    "exception" to e.javaClass.simpleName
                ))
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error processing received fragment", e)
            emitDiagnostic("error", "Error processing received fragment", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }
    
    private fun pruneMeshObservations(now: Long) {
        val iterator = lastSeenMeshAdvertisements.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (now - entry.value.timestamp > MESH_OBSERVATION_TTL_MS) {
                iterator.remove()
            }
        }

        val unknownIterator = unknownBootstrapAttempts.entries.iterator()
        while (unknownIterator.hasNext()) {
            if (now - unknownIterator.next().value > 60_000L) {
                unknownIterator.remove()
            }
        }
    }
    
    // MARK: - Gradient Routing
    
    /** Computes route quality from RSSI value (0.0 to 1.0) */
    private fun computeRouteQuality(rssi: Int?): Float {
        if (rssi == null) return 0.5f
        // Map RSSI from [-100, -20] to [0.0, 1.0]
        val clamped = rssi.coerceIn(-100, -20)
        return (clamped + 100).toFloat() / 80f
    }
    
    /** Learns a route from a received message */
    private fun learnRouteFromMessage(messageJson: String, neighborId: String, neighborAddress: String?) {
        try {
            val json = org.json.JSONObject(messageJson)
            val sender = json.optNullableString("sender") ?: return
            val hopCount = json.optInt("hop_count", 0)
            
            // Don't learn route to ourselves
            if (sender == deviceId) return
            
            // Compute quality from RSSI
            val rssi = neighborAddress?.let { lastSeenRssi[it]?.toInt() }
            val quality = computeRouteQuality(rssi)
            
            // Learn the route: sender can be reached through neighborId (sequence_number from message or 0)
            val seqNum = json.optInt("sequence_number", 0).coerceAtLeast(0).toUInt()
            protocol.learnRoute(
                sender,
                neighborId,
                minOf(255, hopCount + 1).toUByte(),
                quality,
                seqNum
            )
        } catch (e: Exception) {
            Log.w(TAG, "Failed to learn route from message: ${e.message}")
        }
    }
    
    // MARK: - Adaptive Scan Methods
    
    /** Updates the estimated visible peer count based on recent discoveries. */
    private fun updateVisiblePeerCount(now: Long) {
        // Only update periodically to avoid overhead
        if (now - lastPeerCountUpdate < 1000L) {
            return
        }
        lastPeerCountUpdate = now
        
        // Clean up old timestamps and read size inside the same critical
        // section — Collections.synchronizedList's contract requires size /
        // iteration to happen under the same monitor that guards writes.
        val windowStart = now - ADAPTIVE_PEER_COUNT_WINDOW_MS
        val recentCount = synchronized(recentDiscoveryTimestamps) {
            recentDiscoveryTimestamps.removeAll { it < windowStart }
            recentDiscoveryTimestamps.size
        }
        val cachedCount = lastSeenMeshAdvertisements.size
        estimatedVisiblePeerCount = maxOf(recentCount, cachedCount)
    }
    
    /** Records a device discovery for density estimation. */
    private fun recordDiscoveryForDensity(now: Long) {
        recentDiscoveryTimestamps.add(now)
        updateVisiblePeerCount(now)
    }
    
    /** Checks if we should skip this device based on RSSI filtering. */
    private fun shouldFilterByRssi(rssi: Int): Boolean {
        // During aggressive discovery phase, don't apply density-based filtering
        val now = System.currentTimeMillis()
        if (transportStartAt > 0 && now - transportStartAt < AGGRESSIVE_DISCOVERY_PHASE_MS) {
            // Only filter out extremely weak signals during aggressive phase
            return rssi < MINIMUM_RSSI_TO_CONNECT
        }
        
        // In dense networks, apply stricter RSSI filtering
        val threshold = when {
            estimatedVisiblePeerCount > ADAPTIVE_HIGH_DENSITY_THRESHOLD -> -70
            estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD -> ADAPTIVE_MIN_RSSI
            else -> return false // Sparse network - accept all signals
        }
        return rssi < threshold
    }
    
    /** Checks if we should throttle connection attempts based on rate limits. */
    private fun shouldThrottleConnection(address: String, now: Long): Boolean {
        // During aggressive discovery phase, use much shorter cooldowns
        val isAggressivePhase = transportStartAt > 0 && now - transportStartAt < AGGRESSIVE_DISCOVERY_PHASE_MS
        
        // Prune old entries and snapshot the count under the same monitor
        // that guards writes — Collections.synchronizedList's contract
        // requires size / iteration to happen inside a synchronized block.
        val oneMinuteAgo = now - 60_000L
        val globalAttemptCount = synchronized(globalConnectionAttempts) {
            globalConnectionAttempts.removeAll { it < oneMinuteAgo }
            globalConnectionAttempts.size
        }

        val effectiveCooldown = if (isAggressivePhase) 5_000L else ADAPTIVE_COOLDOWN_PER_DEVICE_MS
        deviceConnectionAttempts.entries.removeIf {
            now - it.value >= effectiveCooldown
        }

        // Check per-device cooldown
        val lastAttempt = deviceConnectionAttempts[address]
        if (lastAttempt != null && now - lastAttempt < effectiveCooldown) {
            return true
        }

        // During aggressive phase, allow more connection attempts
        if (isAggressivePhase) {
            // Allow up to 3x the normal rate during aggressive phase
            val maxAttempts = ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE * 3
            if (globalAttemptCount >= maxAttempts) {
                return true
            }
            return false
        }

        // In dense networks, apply global rate limiting
        if (estimatedVisiblePeerCount > ADAPTIVE_LOW_DENSITY_THRESHOLD) {
            if (globalAttemptCount >= ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE) {
                if (logThrottler.shouldLog("adaptive_rate_limit", intervalMs = 5000)) {
                    Log.d(TAG, "Adaptive: rate limiting connections ($globalAttemptCount/$ADAPTIVE_MAX_CONNECTIONS_PER_MINUTE in last minute)")
                }
                return true
            }
        }

        return false
    }
    
    /** Records a connection attempt for rate limiting. */
    private fun recordConnectionAttempt(address: String, now: Long) {
        deviceConnectionAttempts[address] = now
        globalConnectionAttempts.add(now)
    }
    
    /** Returns true if we should apply probabilistic filtering based on network density. */
    private fun shouldProbabilisticallySkip(address: String): Boolean {
        if (estimatedVisiblePeerCount <= ADAPTIVE_LOW_DENSITY_THRESHOLD) {
            return false
        }
        
        // Calculate skip probability based on density
        val density = (estimatedVisiblePeerCount - ADAPTIVE_LOW_DENSITY_THRESHOLD).toDouble()
        val range = (ADAPTIVE_HIGH_DENSITY_THRESHOLD - ADAPTIVE_LOW_DENSITY_THRESHOLD).toDouble()
        val skipProbability = minOf(0.8, density / range * 0.8)
        
        // Use address hash for deterministic selection
        val hash = address.hashCode()
        val normalizedHash = (kotlin.math.abs(hash) % 1000) / 1000.0
        
        return normalizedHash < skipProbability
    }

    private fun addressForNodeHash(nodeHash: Long): String? {
        return lastSeenMeshAdvertisements.entries.firstOrNull {
            it.value.advertisement.nodeIdHash == nodeHash
        }?.key
    }

    private fun maybeHandleRebalance(trigger: String) {
        val directive = meshController.evaluateRebalance() ?: return
        val decision = directive.decision
        val candidateHash = directive.candidate.nodeIdHash
        val candidateAddress = addressForNodeHash(candidateHash)
        if (candidateAddress == null) {
            if (logThrottler.shouldLog("rebalance_missing_candidate", intervalMs = 10_000)) {
                Log.v(TAG, "No address found for rebalance candidate hash=${candidateHash.toString(16)}")
            }
            return
        }

        if (decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, "rebalance_${trigger}")
        }

        if (!meshController.connectionBudgetAvailable() && decision.evictPeerId == null) {
            return
        }

        if (connections.getGatt(candidateAddress) != null) {
            return
        }

        val device = try {
            bluetoothAdapter?.getRemoteDevice(candidateAddress)
        } catch (e: IllegalArgumentException) {
            null
        } ?: return

        val desiredRole = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER, ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }

        connections.setPendingRole(candidateAddress, desiredRole)
        connectToDevice(device)
    }
    
    // MARK: - GATT Server Listener
    //
    // Bridges PeripheralGattServer callbacks into facade state. Callbacks
    // fire on the platform's binder thread; every handler that touches
    // mutable transport state is reposted on [bleHandler] before running,
    // matching the threading model used by start/stop/pause/resume.
    //
    // The `provide*` hooks stay on the binder thread by necessity — they
    // must return bytes synchronously — but they are **pure reads** of
    // @Volatile fields. They must not call into UniFFI or any other path
    // that can block on the protocol mutex, because stalling a GATT binder
    // callback delays every pending operation for that central and risks
    // the system ANR watchdog. Producers (MLS init, advertisement rebuild)
    // call [updateSignedIdentity] on the BLE thread to refresh the cache
    // *before* the read lands.

    private val gattServerListener = object : PeripheralGattServer.Listener {
        override fun onReady() {
            // LeAdvertiser owns its own pending-reason latch; just drain it.
            leAdvertiser.onGattServerReady()
        }

        override fun onSetupFailed(reason: String) {
            Log.e(TAG, "GATT server setup failed: $reason")
            emitDiagnostic(
                "error",
                "gatt_server_setup_failed",
                mapOf("reason" to reason),
            )
            listener?.onTransportError(
                this@BleTransportFacade,
                TransportException.StartFailed("GATT server setup failed: $reason"),
            )
            // Tear the transport down so the caller sees a coherent stopped
            // state. Without this the facade stays in RUNNING while the GATT
            // server is gone, scans keep firing, and every fragment write
            // fails silently.
            bleHandler.post {
                if (state == TransportState.RUNNING || state == TransportState.STARTING) {
                    try {
                        stopUnsafe()
                    } catch (e: Exception) {
                        Log.e(TAG, "Error tearing down after GATT setup failure", e)
                    }
                }
            }
        }

        override fun onCentralConnected(device: BluetoothDevice) {
            if (shuttingDown) return
            bleHandler.post {
                if (shuttingDown) return@post
                handleCentralConnectedOnBleThread(device)
            }
        }

        override fun onCentralDisconnected(device: BluetoothDevice, status: Int) {
            if (shuttingDown) return
            bleHandler.post {
                if (shuttingDown) return@post
                handleCentralDisconnectedOnBleThread(device, status)
            }
        }

        override fun onPeripheralMtuNegotiated(device: BluetoothDevice, maxPayload: Int) {
            // Fired on the GATT-server binder thread when a central renegotiates
            // the MTU on the link it opened to us. Repost to the BLE thread to touch the
            // staging maps and the connection registry without racing the
            // handshake state machine, mirroring onPeerMtuNegotiated.
            if (shuttingDown) return
            bleHandler.post {
                if (shuttingDown) return@post
                assertOnBleThread("onPeripheralMtuNegotiated.stage")
                val address = device.address
                peripheralMaxPayloads[address] = maxPayload
                // Flush now if the peer's device id is already resolved (via our
                // central link's device-id read); otherwise onDeviceIdResolved →
                // flushPeerMtu picks up this staged peripheral payload later.
                val deviceId = connections.deviceIdForAddress(address)
                if (deviceId != null) {
                    flushPeerMtu(address, deviceId)
                }
            }
        }

        override fun onCentralSubscribed(device: BluetoothDevice) {
            // A central enabled notifications on the link it opened to us. Its
            // notify-link MTU may never be reported (server-role onMtuChanged is
            // unreliable), so re-flush now: flushPeerMtu applies the 185-byte
            // notify floor for a subscribed peer whose peripheral MTU is still
            // unknown, bounding multi-fragment notify egress (the MLS Welcome).
            // The CCCD subscribe lands after the central-link MTU flush, so without
            // this the floor would never be applied for this peer until some later
            // unrelated flush. If the device id isn't resolved yet, the
            // onDeviceIdResolved -> flushPeerMtu path applies the floor later (it
            // will see the subscription).
            if (shuttingDown) return
            bleHandler.post {
                if (shuttingDown) return@post
                assertOnBleThread("onCentralSubscribed.reflush")
                val address = device.address
                val deviceId = connections.deviceIdForAddress(address) ?: return@post
                flushPeerMtu(address, deviceId)
            }
        }

        override fun onCentralUnsubscribed(device: BluetoothDevice) {
            // A central disabled notifications on the link it opened to us. The peer
            // is no longer notify-reachable, so re-flush: flushPeerMtu drops the
            // 185-byte notify floor now that subscribedNotifyAddressFor no longer
            // resolves, relaxing the per-peer MTU back to min(central, peripheral) —
            // or clearing it when neither direction's MTU is known. Without this, a
            // peer that subscribed (pinning the floor) and then unsubscribed on a
            // live link would keep the stale 185 floor, throttling its central-write
            // egress, until some later unrelated flush. Symmetric with
            // onCentralSubscribed.
            if (shuttingDown) return
            bleHandler.post {
                if (shuttingDown) return@post
                assertOnBleThread("onCentralUnsubscribed.reflush")
                val address = device.address
                val deviceId = connections.deviceIdForAddress(address) ?: return@post
                flushPeerMtu(address, deviceId)
            }
        }

        override fun onInboundFragment(device: BluetoothDevice, bytes: ByteArray) {
            if (shuttingDown) return
            Log.i(TAG, "MESSAGE CHARACTERISTIC WRITE from ${device.address}, processing...")
            emitDiagnostic(
                "info",
                "GATT write request received",
                mapOf(
                    "deviceAddress" to device.address,
                    "dataSize" to bytes.size,
                ),
            )
            val address = device.address
            bleHandler.post {
                if (shuttingDown) return@post
                handleReceivedData(bytes, address)
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            if (shuttingDown) return
            // Our peripheral notify to this central completed — release the
            // per-peer write gate and pump the next fragment, exactly like
            // onCharacteristicWrite does for the central write path. Makes
            // multi-fragment notify replies event-paced (fast) instead of
            // waiting on the 30ms watchdog for every fragment.
            val address = device.address
            // Diagnostic: proves the stack actually transmitted the notification
            // (status 0 = GATT_SUCCESS). If "Replying via notify" appears but this
            // never does, the notify is stuck in the stack queue / not going OTA.
            Log.i(TAG, "Notification flushed to $address status=$status")
            bleHandler.post {
                if (shuttingDown) return@post
                onWriteCompleted(address)
            }
        }

        override fun provideDeviceIdBytes(device: BluetoothDevice): ByteArray? {
            if (shuttingDown) return null
            // Pure volatile read, for the same reason as provideIdentityBytes:
            // this is a binder thread and `protocol.localAddress()` would take
            // the protocol mutex, stalling every pending GATT op for this
            // central.
            //
            // Null until MLS is initialized. Failing the read is the correct
            // answer — a peer that cannot bind our id to a key must not
            // surface us at all — and it is self-healing: the same refresh
            // that primes the identity cache primes this, and the central
            // retries on its next connection.
            val address = cachedLocalAddress
            if (address == null) {
                bleHandler.post { ensureIdentityRefreshScheduled() }
                return null
            }
            Log.d(TAG, "Sent device ID to ${device.address}")
            return address.toByteArray(Charsets.UTF_8)
        }

        override fun provideIdentityBytes(device: BluetoothDevice): ByteArray? {
            if (shuttingDown) return null
            // Pure volatile read. Never call updateSignedIdentity() here —
            // it would block this binder thread on the protocol mutex.
            // If the cache isn't primed yet, return null; the central will
            // retry. See the comment on [updateSignedIdentity].
            val identity = cachedSignedIdentity?.encode()
            if (identity == null) {
                // Cache isn't primed yet — arrange a bounded-backoff refresh
                // on the BLE thread so the next central read eventually
                // succeeds instead of spinning forever on a permanent null.
                bleHandler.post { ensureIdentityRefreshScheduled() }
                return null
            }
            Log.d(TAG, "Sent signed identity to ${device.address}")
            return identity
        }
    }

    private fun handleCentralConnectedOnBleThread(device: BluetoothDevice) {
        val observation = lastSeenMeshAdvertisements[device.address]
        val decision = meshController.shouldAcceptInboundConnection(
            connections.deviceIdForAddress(device.address),
            observation?.advertisement,
            observation?.rssi
        )
        if (decision.evictPeerId != null) {
            evictPeer(decision.evictPeerId, "inbound_swap")
        }
        if (decision.intent == ConnectionIntent.REJECTED) {
            Log.w(TAG, "Rejecting inbound connection from ${device.address}: ${decision.reason}")
            peripheralGattServer?.cancelConnection(device)
            return
        }
        // Check connection capacity
        if (currentConnectionCount() >= MAX_CONNECTIONS_PER_DEVICE) {
            Log.w(TAG, "Rejecting inbound connection from ${device.address}: connection cap reached")
            peripheralGattServer?.cancelConnection(device)
            return
        }
        val role = when (decision.intent) {
            ConnectionIntent.INTER_CLUSTER -> MeshRole.BRIDGE
            ConnectionIntent.INTRA_CLUSTER, ConnectionIntent.REJECTED -> MeshRole.MEMBER
        }
        connections.trackServerConnection(device.address)
        connections.setPendingRole(device.address, role)
        Log.i(TAG, "GATT server: Device connected: ${device.address} (role=$role)")
        emitDiagnostic("info", "Device connected to GATT server", mapOf("address" to device.address))
    }

    private fun handleCentralDisconnectedOnBleThread(device: BluetoothDevice, status: Int) {
        val address = device.address
        connections.untrackServerConnection(address)
        connections.consumePendingRole(address)
        // Status 0 = clean local disconnect; status 19 (0x13) =
        // HCI_CONN_TERMINATE_PEER_USER, i.e. the remote end disconnected
        // cleanly. Both are normal lifecycle events where the peer is
        // likely to come back, so we keep the address→deviceId mapping and
        // RSSI cached to speed up reconnection. Any other status is a real
        // failure and we tear the peer state down.
        //
        // MTU state invariant: this handler MUST NOT clear or demote the
        // CENTRAL-owned `peer_mtus` value or its staging slot
        // (`peerMaxPayloads`). It MAY drop the peripheral/NOTIFY-link slot
        // (`peripheralMaxPayloads`) — that link IS this connection — and
        // re-flush the recomputed min, which only ever RAISES the per-peer
        // MTU back toward the central value (see the clean-disconnect block
        // below). This function runs on a peripheral-role
        // disconnect (a remote central disconnected from our GATT
        // server). The `peer_mtus[deviceId]` entry is owned by our
        // *central-role* link to the same peer, populated via
        // `CentralGattClient.onMtuChanged → flushPeerMtu`. A prior
        // version of this handler called `bleClearPeerMtu` on clean
        // disconnect under the mistaken assumption that clearing was
        // needed to defend against a stale-MTU-on-reconnect race — but
        // that race is owned by the central-role reconnect flow, which
        // handles it locally by letting the next `onMtuChanged`
        // overwrite the entry. Clearing here demoted an alive central-
        // role link from its negotiated BLE 5 MTU back to the 185-byte
        // floor for the rest of the link's life and made every
        // subsequent send to that peer tick `fragment_fallback_count`.
        // The non-clean branch below calls `blePeerLost` — a
        // pre-existing behavior deliberately left in place, since
        // deciding whether a peripheral-role disconnect should imply
        // central-role peer loss is a protocol-level question outside
        // the scope of this fix. `blePeerLost` in that branch drops
        // `peer_mtus` via `on_peer_lost`, which is correct for a path
        // that *also* drops the peer from the Rust `peers` map.
        val isCleanDisconnect = status == 0 || status == 19
        // The peripheral/NOTIFY link to this central is gone. Drop its payload
        // and, on a clean disconnect (peer likely returns, deviceId mapping
        // kept), recompute the per-peer MTU from the surviving central link —
        // restoring it from any min() demotion the notify link imposed. On a
        // non-clean disconnect the teardown below calls blePeerLost (drops
        // peer_mtus wholesale) + dropStagedPeerMtu, so we defer to that path.
        if (isCleanDisconnect && peripheralMaxPayloads.remove(address) != null) {
            connections.deviceIdForAddress(address)?.let { peerId ->
                // Drop the per-device peripheral slot too, or the recompute would
                // re-min against the dead notify link's (smaller) bound. With it
                // gone, flushPeerMtu restores the per-peer MTU from the surviving
                // central link's per-device slot.
                peripheralPayloadByDevice.remove(peerId)
                flushPeerMtu(address, peerId)
            }
        }
        if (!isCleanDisconnect) {
            lastSeenRssi.remove(address)
            connections.deviceIdForAddress(address)?.let { peerId ->
                protocol.removeNeighborRoutes(peerId)
                try {
                    protocol.blePeerLost(peerId)
                } catch (e: Exception) {
                    Log.e(TAG, "Error notifying peer lost", e)
                    emitDiagnostic("error", "Error notifying peer lost", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                }
                // `blePeerLost` above already dropped the Rust-side MTU
                // entry via `on_peer_lost`; only the facade-side staged
                // slot (keyed by BLE address) still needs clearing here.
                dropStagedPeerMtu(address, peerId)
                meshController.registerDisconnection(peerId)
                refreshSelfMetrics()
                connections.removeIdentifiersForAddress(address)
                centralClient.clearResolutionAttempt(address)
                connections.removeConnectionRole(peerId)
                if (state == TransportState.RUNNING) {
                    refreshAdvertising("membership_change")
                }
                maybeHandleRebalance("disconnect")
            }
        }
    }

}
