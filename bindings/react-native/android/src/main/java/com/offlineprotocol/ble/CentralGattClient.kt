package com.offlineprotocol.ble

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.offlineprotocol.PeerIdentityBinding
import com.offlineprotocol.mesh.MeshController
import com.offlineprotocol.mesh.SignedIdentityData
import uniffi.offline_protocol.OfflineProtocol
import uniffi.offline_protocol.deriveAddress
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * Central-role BLE GATT client extracted from [BleTransportFacade]. Owns:
 *
 *   - The shared [BluetoothGattCallback] plugged into every `connectGatt`
 *     call via [callback]. One callback instance handles every outbound
 *     client link; per-address state is looked up from the maps below on
 *     each callback entry.
 *
 *   - The per-address handshake state machine:
 *       `onConnectionStateChange(CONNECTED)` → `discoverServices()`
 *       `onServicesDiscovered` → `requestMtu(REQUESTED_ATT_MTU)`
 *       `onMtuChanged` → report payload to host, `readCharacteristic(DEVICE_ID)`
 *       `onCharacteristicRead(DEVICE_ID)` → `readCharacteristic(IDENTITY)`
 *       `onCharacteristicRead(IDENTITY)` → `setCharacteristicNotification`
 *          + `writeDescriptor(CCCD)`
 *       `onDescriptorWrite(CCCD success)` → mark link ready, drain outbound
 *     Firing more than one GATT op per callback entry is deliberately
 *     avoided — Android's BLE stack serialises one op at a time and some
 *     vendors silently drop a second op issued in the same tick, which
 *     produces a "connection looks healthy but no bytes flow" failure mode.
 *
 *   - [linkReady]: addresses whose CCCD subscribe write has been acked by
 *     the remote peripheral. Until the write lands we cannot trust that
 *     notifications will flow, so outbound writes defer via the fragment
 *     queue.
 *
 *   - [connectionRetryCount]: per-address reconnect backoff counter driving
 *     exponential-backoff reconnects on `STATE_DISCONNECTED` with status
 *     other than clean local/peer disconnect.
 *
 *   - [deviceIdResolutionAttempts]: per-address throttle for reverse GATT
 *     reads that resolve a peer's stable device id after an inbound write
 *     or notification from an unknown address. Used by both the central
 *     callback and the facade's connection monitor.
 *
 * Concurrency: binder-thread GATT callbacks either (a) snapshot the state
 * they need and repost to main for further processing, matching the
 * threading model used by [PeripheralGattServer], or (b) early-return on
 * the [Host.isShuttingDown] gate. No UniFFI call is ever issued from a
 * binder callback thread — every protocol entry happens on main.
 */
internal class CentralGattClient(
    private val mainHandler: Handler,
    private val serviceUuid: UUID,
    private val messageCharUuid: UUID,
    private val deviceIdCharUuid: UUID,
    private val identityCharUuid: UUID,
    private val host: Host,
    private val diagnosticEmitter: (level: String, message: String, ctx: Map<String, Any?>) -> Unit =
        { _, _, _ -> },
) {

    /**
     * Callbacks the client needs to reach back into the orchestrating
     * transport. Intentionally explicit (not "give me the facade") so the
     * coupling surface is reviewable and the client can be unit-tested
     * against a fake host.
     */
    interface Host {
        val protocol: OfflineProtocol
        val connections: MeshConnectionRegistry
        val pendingInbound: InboundFragmentBuffer
        val outboundQueue: OutboundFragmentQueue
        val meshController: MeshController
        val bluetoothAdapter: BluetoothAdapter?
        val selfDeviceId: String

        fun isShuttingDown(): Boolean
        fun isRunning(): Boolean

        fun rssiFor(address: String): Short?
        fun clearRssi(address: String)
        fun markNonMeshDevice(address: String)

        fun refreshAdvertising(reason: String)
        fun refreshSelfMetrics()
        fun maybeHandleRebalance(trigger: String)
        fun learnRouteFromMessage(messageJson: String, neighborId: String, neighborAddress: String?)
        fun drainAndSendFragments()

        /** Release the per-peer write gate after a GATT characteristic write
         *  to [address] completes (the stack flushed the op and will accept
         *  the next write). On API 33+ the value-overload of
         *  writeCharacteristic rejects a concurrent write with
         *  ERROR_GATT_WRITE_REQUEST_BUSY (201) while one is still outstanding,
         *  even for WRITE_TYPE_NO_RESPONSE, so the facade serialises one
         *  outstanding write per peer and uses this signal to drain the next
         *  fragment. Called on the main thread. */
        fun onWriteCompleted(address: String)

        /** Hand a received fragment to the facade (either via client
         *  notify or via central-side write). Called on the main thread. */
        fun handleInboundFragment(address: String, data: ByteArray)

        /** Notify the facade that a remote peripheral's ATT MTU negotiation
         *  has completed. [maxPayload] is the already header-adjusted max
         *  fragment size (ATT MTU minus the 3-byte header). Called from the
         *  binder GATT thread; implementations must repost to main before
         *  touching UniFFI. */
        fun onPeerMtuNegotiated(address: String, maxPayload: Int)

        /** Notify the facade that a remote peripheral's device ID has been
         *  resolved via a reverse GATT read. Called on the main thread,
         *  immediately after [MeshConnectionRegistry.setDeviceIdentifier] has
         *  been populated for [address]. Used by the facade to flush any
         *  MTU value previously staged under [address] into the Rust
         *  transport keyed by [deviceId]. */
        fun onDeviceIdResolved(address: String, deviceId: String)

        /** Notify the facade that a peer has been permanently given up on
         *  (retry budget exhausted or stale callback post-teardown). Called
         *  from [finalizeGivenUpPeer] on the main thread — the facade uses
         *  this to drop facade-only state that is not already cleared via
         *  [clearPeerBuffers] (the staged negotiated-MTU map, the Rust-side
         *  per-peer MTU entry). */
        fun onPeerGivenUp(address: String, peerId: String)

        /** Entry point used by the retry-on-disconnect path to re-attempt
         *  connecting to a known address. Facade enforces the per-device
         *  RSSI / capacity / cooldown gating inside connectToDevice. */
        fun connectToDevice(device: BluetoothDevice)
    }

    companion object {
        private const val TAG = "CentralGattClient"
        private const val MAX_CONNECTION_RETRIES = 5
        private const val MIN_RECONNECT_INTERVAL_MS = 5_000L
        private const val MAX_RECONNECT_INTERVAL_MS = 60_000L
        /**
         * Android's catch-all GATT failure status (0x85). The BLE stack
         * reports it for a grab-bag of low-level failures — LL timeout,
         * controller reset, failed bonding, resource starvation — without
         * distinguishing them. We treat it as "something about this
         * address's signal quality is currently unreliable" and clear the
         * cached RSSI so the adaptive-scan gate has to re-learn the peer
         * from a fresh advertisement before we try again. Other status
         * codes (e.g. 8 = CONNECTION_TIMEOUT, 19 = REMOTE_USER_TERMINATED)
         * leave RSSI intact because they do not imply a quality problem.
         */
        private const val GATT_GENERIC_ERROR = 133
        /** Hard ceiling on a single inbound BLE notification we accept from
         *  a remote peripheral. Mirrors [PeripheralGattServer.MAX_INBOUND_WRITE_BYTES]
         *  — well above mesh fragment size for MTU-negotiation headroom, but
         *  bounded so a hostile peripheral cannot balloon memory through
         *  the notify path. */
        private const val MAX_INBOUND_NOTIFICATION_BYTES = 4096
        /** Re-drain iteration cap inside [drainPendingInboundFor] so a
         *  pathologically chatty peer racing with the drain loop cannot
         *  pin the main thread indefinitely. */
        private const val MAX_DRAIN_ITERATIONS = 8
        /** ATT MTU we ask for in `requestMtu`. 517 is the BLE 5 spec max;
         *  controllers negotiate down to whatever both peers support. The
         *  resulting payload = mtu − 3 is reported to Rust so the
         *  fragmenter can size outbound chunks per peer. */
        private const val REQUESTED_ATT_MTU = 517
        /** ATT protocol overhead (opcode + attribute handle) subtracted
         *  from the negotiated MTU to get usable Write-Without-Response
         *  payload. */
        private const val ATT_HEADER_BYTES = 3
        /** Hard deadline for a `requestMtu` → `onMtuChanged` round-trip,
         *  in milliseconds. Some Android BLE stacks are known to accept
         *  `requestMtu` (returning true) and then never deliver the
         *  callback on flaky peripherals, leaving the handshake chain
         *  wedged until GATT supervision timeout (~20s). Well above the
         *  typical 100–500ms negotiation window — a ~10× safety margin —
         *  and short enough that a hung link surfaces quickly. If the
         *  deadline passes, the watchdog resumes the handshake with
         *  whatever MTU the controller auto-negotiated, and the Rust
         *  transport falls back to its 185-byte fragment floor for that
         *  peer. */
        private const val MTU_WATCHDOG_MS = 3000L
    }

    // Addresses with an in-flight `requestMtu` whose `onMtuChanged`
    // callback has not yet landed. Entry is inserted just before
    // `gatt.requestMtu`, removed atomically by whichever of {real callback,
    // watchdog runnable} fires first — the loser sees `remove(...) == false`
    // and bails out. Also cleared from every handshake-teardown path
    // (service missing, permission denied, finalizeGivenUpPeer) so a stale
    // watchdog does not fire against a gone peer.
    private val mtuInFlight = ConcurrentHashMap<String, Boolean>()

    // Pending watchdog Runnables keyed by address, so they can be cancelled
    // via `mainHandler.removeCallbacks` when the real `onMtuChanged` lands
    // first. Separate from [mtuInFlight] because `ConcurrentHashMap<_, Unit>`
    // does not model "present-or-absent" cleanly.
    private val mtuWatchdogs = ConcurrentHashMap<String, Runnable>()

    // Addresses whose CCCD subscription has been acked by the remote peer.
    // Until the descriptor write lands we can't trust that the peripheral
    // is actually notifying us, so the link is only considered ready to
    // drain fragments once this set contains the address. Snapshot-safe
    // for binder-thread readers because it's backed by a ConcurrentHashMap.
    private val linkReady = ConcurrentHashMap.newKeySet<String>()

    private val connectionRetryCount = ConcurrentHashMap<String, Int>()

    private val deviceIdResolutionAttempts = ConcurrentHashMap<String, Long>()

    /**
     * What a peer advertised in `DEVICE_ID`, held between that read and the
     * `IDENTITY` read that either proves it or refuses it.
     *
     * An entry here is an unproven claim and is never announced. It is read by
     * [handleIdentityRead] and removed by [rejectPeer] and by every teardown
     * path, so a reconnect cannot pair this connection's device id with the
     * next connection's identity. (A verified entry is left in place for the
     * life of the link rather than consumed — it is keyed by device address,
     * so it is overwritten by the next connection's own read.)
     */
    private val advertisedDeviceIds = ConcurrentHashMap<String, String>()

    /** True if a successful CCCD write has been observed for [address]. */
    fun isLinkReady(address: String): Boolean = linkReady.contains(address)

    /** Drop link-ready, retry, and device-id state for [address]. */
    fun forgetLink(address: String) {
        linkReady.remove(address)
        connectionRetryCount.remove(address)
        deviceIdResolutionAttempts.remove(address)
        advertisedDeviceIds.remove(address)
        cancelMtuWatchdog(address)
    }

    /** Drop all per-address state. Used by transport stop. */
    fun clearAll() {
        linkReady.clear()
        connectionRetryCount.clear()
        deviceIdResolutionAttempts.clear()
        advertisedDeviceIds.clear()
        mtuInFlight.clear()
        mtuWatchdogs.values.forEach { mainHandler.removeCallbacks(it) }
        mtuWatchdogs.clear()
    }

    /**
     * Cancels any pending MTU watchdog for [address] and drops the
     * associated `mtuInFlight` slot. Called from every handshake
     * teardown path so a stale watchdog cannot fire against a gone peer
     * (which would race `closeGattClient`, re-enter the chain, and log
     * confusing "service missing" warnings).
     */
    private fun cancelMtuWatchdog(address: String) {
        mtuInFlight.remove(address)
        mtuWatchdogs.remove(address)?.let { mainHandler.removeCallbacks(it) }
    }

    fun markResolutionAttempt(address: String, now: Long) {
        deviceIdResolutionAttempts[address] = now
    }

    fun lastResolutionAttempt(address: String): Long? = deviceIdResolutionAttempts[address]

    fun clearResolutionAttempt(address: String) {
        deviceIdResolutionAttempts.remove(address)
    }

    // --- Helpers ---

    private fun runOnMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            action()
        } else {
            mainHandler.post(action)
        }
    }

    // --- Callback ---

    val callback: BluetoothGattCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            if (host.isShuttingDown()) {
                // A stale callback landing mid-teardown must not touch the
                // shared state the main thread has just cleared. Close the
                // gatt handle locally so the stack releases its LL slot.
                try { gatt.close() } catch (_: Exception) {}
                return
            }
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> handleConnected(gatt)
                BluetoothProfile.STATE_DISCONNECTED -> handleDisconnected(gatt, status)
            }
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            if (host.isShuttingDown()) {
                try { gatt.close() } catch (_: Exception) {}
                return
            }
            val address = gatt.device.address
            Log.i(TAG, "onServicesDiscovered: $address, status=$status (${if (status == BluetoothGatt.GATT_SUCCESS) "SUCCESS" else "FAILED"})")

            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.e(TAG, "Service discovery FAILED for $address, status=$status")
                diagnosticEmitter("error", "Service discovery failed", mapOf("address" to address, "status" to status))
                closeGattClient(gatt, "service_discovery_failed")
                return
            }

            val service = gatt.getService(serviceUuid)
            if (service == null) {
                Log.w(TAG, "Service UUID not found on $address. Available services: ${gatt.services.map { it.uuid }}")
                diagnosticEmitter("warning", "Offline protocol service not found", mapOf(
                    "address" to address,
                    "serviceCount" to gatt.services.size,
                ))
                host.markNonMeshDevice(address)
                closeGattClient(gatt, "no_mesh_service")
                return
            }

            Log.i(TAG, "Found offline protocol service on $address")
            diagnosticEmitter("info", "GATT services discovered", mapOf("address" to address))

            // Kick off the first GATT op only. Android's BLE stack serialises
            // one GATT op at a time, so we chain:
            //   onServicesDiscovered → requestMtu(REQUESTED_ATT_MTU)
            //   onMtuChanged → readCharacteristic(deviceId)
            //   onCharacteristicRead(deviceId) → readCharacteristic(identity)
            //   onCharacteristicRead(identity) → setNotification + writeDescriptor(CCCD)
            //   onDescriptorWrite(CCCD success) → mark linkReady + drain
            // Firing more than one op here causes the second to be silently
            // dropped on some vendors, which is the class of bug where the
            // connection "looks healthy" but no messages flow. MTU negotiation
            // runs first so that by the time we drain outbound fragments
            // (after CCCD), the Rust fragmenter already knows the per-peer
            // payload size and can size fragments to the real link capacity
            // instead of the 185-byte fallback floor.
            requestPeerMtuOrResumeChain(gatt, service, address)
        }

        /**
         * Arms the MTU watchdog, issues `requestMtu`, and handles the
         * synchronous-false / permission-denied fallbacks. Extracted from
         * [onServicesDiscovered] so the handshake state machine in that
         * callback reads as a linear chain rather than 40+ lines of
         * watchdog arming and error branches inline. The happy path
         * (`requestMtu` returned true, real `onMtuChanged` fires) resumes
         * via [onMtuChanged]; the error paths resume via
         * [readDeviceIdCharacteristicOrClose] under the controller-default
         * MTU. Called from the GATT binder callback thread (the same
         * thread as the rest of [onServicesDiscovered]); the watchdog
         * Runnable it arms runs on [mainHandler] instead, and the two
         * race to claim the `mtuInFlight` slot via ConcurrentHashMap CAS.
         */
        private fun requestPeerMtuOrResumeChain(
            gatt: BluetoothGatt,
            service: BluetoothGattService,
            address: String,
        ) {
            // Arm the MTU watchdog BEFORE requestMtu so that a racing
            // fire from `mainHandler.postDelayed` always sees an entry in
            // [mtuInFlight]. On success this is cancelled from the top of
            // [onMtuChanged]; on the synchronous-false / permission-denied
            // fallbacks below we cancel explicitly before taking the
            // alternative path.
            mtuInFlight[address] = true
            val watchdog = Runnable {
                if (host.isShuttingDown()) return@Runnable
                // CAS: watchdog only resumes the chain if no real callback
                // has already claimed this address. `remove() != null`
                // expresses the atomic present-or-absent semantics more
                // clearly than the Boolean-autoboxed `== true` pattern.
                val claimed = mtuInFlight.remove(address) != null
                if (!claimed) return@Runnable
                mtuWatchdogs.remove(address)
                Log.w(TAG, "MTU watchdog fired for $address after ${MTU_WATCHDOG_MS}ms — resuming handshake without negotiated MTU")
                diagnosticEmitter(
                    "warning",
                    "MTU watchdog fired",
                    mapOf("address" to address, "timeoutMs" to MTU_WATCHDOG_MS),
                )
                // Re-entrancy note: this block runs on the main handler
                // while an `onMtuChanged` callback for the same `gatt`
                // may still be dispatched by the binder GATT thread
                // moments later. That race is safe because:
                //   - The `mtuInFlight.remove(...) != null` CAS above
                //     means a concurrent onMtuChanged will observe
                //     `claimed == false` and divert to the late-
                //     callback branch (which re-gates on
                //     `getGatt(address) != null`), rather than re-
                //     entering the handshake chain under our feet.
                //   - `closeGattClient` tolerates being called while
                //     the binder thread holds a reference to the same
                //     `gatt` instance — Android's BLE stack serialises
                //     GATT operations per-client internally, and
                //     `closeGattClient` only manipulates our own
                //     per-address registry + calls `gatt.close()`,
                //     which is documented as thread-safe.
                //   - `readDeviceIdCharacteristicOrClose` issues exactly
                //     one GATT op, which the stack queues behind any
                //     still-in-flight op on the same gatt.
                // Re-fetch the service — the cached local may still be
                // valid, but going through `gatt.getService` is resilient
                // if the stack tore it down underneath us.
                val currentService = gatt.getService(serviceUuid)
                if (currentService == null) {
                    closeGattClient(gatt, "service_missing_post_mtu_watchdog")
                } else {
                    readDeviceIdCharacteristicOrClose(gatt, currentService, address)
                }
            }
            mtuWatchdogs[address] = watchdog
            mainHandler.postDelayed(watchdog, MTU_WATCHDOG_MS)

            try {
                val mtuRequested = gatt.requestMtu(REQUESTED_ATT_MTU)
                if (!mtuRequested) {
                    Log.w(TAG, "requestMtu returned false for $address; continuing with default MTU")
                    diagnosticEmitter("warning", "requestMtu returned false", mapOf("address" to address))
                    // Cancel the watchdog we just armed — we already know
                    // the callback will never arrive, so fall through to
                    // the device-id read under the controller-default MTU.
                    cancelMtuWatchdog(address)
                    readDeviceIdCharacteristicOrClose(gatt, service, address)
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "Permission denied requesting MTU", e)
                diagnosticEmitter("error", "Permission denied requesting MTU", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                cancelMtuWatchdog(address)
                closeGattClient(gatt, "mtu_request_permission_denied")
            }
        }

        override fun onMtuChanged(gatt: BluetoothGatt, mtu: Int, status: Int) {
            if (host.isShuttingDown()) return
            val address = gatt.device.address
            // Claim the in-flight slot atomically. If the watchdog has
            // already fired (and removed the entry) we're the loser of
            // that race — the chain has already been resumed, so drop
            // this late callback on the floor.
            val claimed = mtuInFlight.remove(address) != null
            if (!claimed) {
                // Watchdog fired first and already resumed the handshake at
                // the fallback fragment size. Don't re-enter the chain, but
                // still forward a successful negotiation so the facade can
                // flush the real payload size into Rust — see
                // BleTransportFacade.onPeerMtuNegotiated, which stages the
                // payload and, if the device id has already been resolved
                // for this address, calls flushPeerMtu immediately.
                // Dropping the value here would pin this link to the
                // 185-byte floor for its remaining lifetime.
                //
                // Gate the forward on the gatt client still being
                // registered for this address: if teardown has already
                // run (closeGattClient → removeGatt, or finalizeGivenUpPeer,
                // or stop), forwarding the stage would leak an entry in
                // the facade's `peerMaxPayloads` map that no subsequent
                // teardown path would clear — a reconnect on the same
                // BLE address could then observe a stale value from the
                // previous session. `getGatt(address) != null` is true
                // throughout normal handshake (including the
                // post-watchdog, pre-CCCD window) and flips false the
                // moment `closeGattClient` runs.
                val stillConnected = host.connections.getGatt(address) != null
                if (status == BluetoothGatt.GATT_SUCCESS && stillConnected) {
                    val maxPayload = (mtu - ATT_HEADER_BYTES).coerceAtLeast(0)
                    Log.i(TAG, "Late onMtuChanged for $address after watchdog: mtu=$mtu payload=$maxPayload — forwarding to facade")
                    diagnosticEmitter(
                        "info",
                        "MTU negotiated (late)",
                        mapOf("address" to address, "mtu" to mtu, "payload" to maxPayload),
                    )
                    host.onPeerMtuNegotiated(address, maxPayload)
                } else {
                    Log.i(TAG, "Late onMtuChanged for $address after watchdog — status=$status stillConnected=$stillConnected, ignoring")
                }
                return
            }
            mtuWatchdogs.remove(address)?.let { mainHandler.removeCallbacks(it) }
            if (status == BluetoothGatt.GATT_SUCCESS) {
                val maxPayload = (mtu - ATT_HEADER_BYTES).coerceAtLeast(0)
                Log.i(TAG, "MTU negotiated for $address: mtu=$mtu payload=$maxPayload")
                diagnosticEmitter(
                    "info",
                    "MTU negotiated",
                    mapOf("address" to address, "mtu" to mtu, "payload" to maxPayload),
                )
                host.onPeerMtuNegotiated(address, maxPayload)
            } else {
                Log.w(TAG, "MTU negotiation failed for $address, status=$status — using default")
                diagnosticEmitter(
                    "warning",
                    "MTU negotiation failed",
                    mapOf("address" to address, "status" to status),
                )
                // Leave the peer without a stored MTU; Rust will use its
                // fallback fragment size and outbound writes stay within
                // the controller-default ATT MTU.
            }

            // Resume the handshake chain regardless of MTU negotiation
            // outcome — a missing MTU is recoverable (fallback fragment
            // size), but skipping the device-id read is not.
            val service = gatt.getService(serviceUuid)
            if (service == null) {
                Log.w(TAG, "Service disappeared between discover and MTU ack for $address")
                diagnosticEmitter("warning", "Service missing after MTU negotiation", mapOf("address" to address))
                closeGattClient(gatt, "service_missing_post_mtu")
                return
            }
            readDeviceIdCharacteristicOrClose(gatt, service, address)
        }

        /**
         * Kicks off `readCharacteristic(deviceId)`. Extracted so both
         * [onServicesDiscovered] (in the `requestMtu returned false` path)
         * and [onMtuChanged] can resume the handshake chain without
         * duplicating the error handling.
         */
        private fun readDeviceIdCharacteristicOrClose(
            gatt: BluetoothGatt,
            service: BluetoothGattService,
            address: String,
        ) {
            val deviceIdChar = service.getCharacteristic(deviceIdCharUuid)
            if (deviceIdChar == null) {
                Log.w(TAG, "Device ID characteristic NOT FOUND on $address")
                diagnosticEmitter("warning", "Device ID characteristic missing", mapOf("address" to address))
                closeGattClient(gatt, "device_id_char_missing")
                return
            }

            Log.d(TAG, "Reading device ID characteristic from $address")
            try {
                val readStarted = gatt.readCharacteristic(deviceIdChar)
                if (!readStarted) {
                    Log.w(TAG, "readCharacteristic(deviceId) returned false for $address")
                    diagnosticEmitter("warning", "readCharacteristic(deviceId) returned false", mapOf("address" to address))
                    closeGattClient(gatt, "device_id_read_rejected")
                }
            } catch (e: SecurityException) {
                Log.e(TAG, "Permission denied reading characteristic", e)
                diagnosticEmitter("error", "Permission denied reading device ID characteristic", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
                closeGattClient(gatt, "device_id_read_permission_denied")
            }
        }

        override fun onCharacteristicRead(
            gatt: BluetoothGatt,
            characteristic: BluetoothGattCharacteristic,
            status: Int,
        ) {
            if (host.isShuttingDown()) return
            // Snapshot the payload on the binder thread before the BLE
            // stack reuses the buffer for the next operation, then repost
            // the entire body to the main handler. Everything downstream
            // from here calls into UniFFI (blePeerDiscovered,
            // bleFragmentReceived, verifySignature, learnRoute) and must
            // never run on the shared binder-thread pool — stalling it
            // blocks GATT work for every client in this process. Same
            // pattern as onCharacteristicChanged below.
            val charUuid = characteristic.uuid
            @Suppress("DEPRECATION")
            val snapshot = characteristic.value?.copyOf()
            mainHandler.post {
                if (host.isShuttingDown()) return@post
                handleCharacteristicReadOnMain(gatt, charUuid, snapshot, status)
            }
        }

        override fun onDescriptorWrite(gatt: BluetoothGatt, descriptor: BluetoothGattDescriptor, status: Int) {
            if (host.isShuttingDown()) return
            val address = gatt.device.address
            if (descriptor.uuid != PeripheralGattServer.CCCD_UUID) {
                return
            }
            val success = status == BluetoothGatt.GATT_SUCCESS
            if (!success) {
                Log.w(TAG, "CCCD write failed for $address, status=$status; tearing link down")
                diagnosticEmitter("warning", "CCCD write failed", mapOf(
                    "address" to address,
                    "status" to status,
                ))
                closeGattClient(gatt, "cccd_write_failed")
                return
            }
            // Ack arrives on the binder thread. The previous shape added
            // `address` to [linkReady] here and only then posted the drain
            // to main — that opened a race where a concurrent
            // `evictPeer → forgetLink(address)` running on main could
            // remove the address *before* this binder-thread `add`
            // reinstated it, leaving a zombie linkReady entry pointing at
            // a gatt the facade has already torn down. Moving both the
            // mutation and the drain into the same main-thread post makes
            // evict and CCCD-ack mutually serialisable on the looper.
            mainHandler.post {
                if (host.isShuttingDown()) return@post
                Log.i(TAG, "CCCD write acknowledged for $address — link ready")
                diagnosticEmitter("info", "BLE link ready", mapOf("address" to address))
                linkReady.add(address)
                if (host.isRunning()) {
                    host.drainAndSendFragments()
                }
            }
        }

        override fun onCharacteristicWrite(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, status: Int) {
            if (host.isShuttingDown()) return
            if (characteristic.uuid != messageCharUuid) return
            // Release the per-peer write gate and drive the next fragment. Done
            // even on a non-success status so a failed write cannot leave the
            // gate stuck and wedge the peer; the facade re-drives from the queue.
            val address = gatt.device.address
            mainHandler.post {
                if (host.isShuttingDown()) return@post
                host.onWriteCompleted(address)
            }
        }

        override fun onCharacteristicChanged(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic) {
            if (host.isShuttingDown()) return
            if (characteristic.uuid != messageCharUuid) return
            // Snapshot the value on the binder thread — characteristic.value
            // is overwritten by the BLE stack on the next notification —
            // then repost processing to the main thread so we never hold up
            // a binder callback on the UniFFI mutex. The .copyOf() is
            // load-bearing: without it the main-thread handler reads whatever
            // bytes happen to be in the framework buffer when it eventually
            // runs, which is the *next* notification under burst load.
            @Suppress("DEPRECATION")
            val raw = characteristic.value ?: return
            if (raw.size > MAX_INBOUND_NOTIFICATION_BYTES) {
                // Defence in depth against a hostile peripheral. Mirrors the
                // peripheral-side cap in PeripheralGattServer.MAX_INBOUND_WRITE_BYTES.
                Log.w(TAG, "Dropping oversized inbound notification: ${raw.size} bytes from ${gatt.device.address}")
                diagnosticEmitter(
                    "warning",
                    "Dropped oversized inbound BLE notification",
                    mapOf(
                        "address" to gatt.device.address,
                        "size" to raw.size,
                        "max" to MAX_INBOUND_NOTIFICATION_BYTES,
                    ),
                )
                return
            }
            val data = raw.copyOf()
            val address = gatt.device.address
            mainHandler.post {
                if (host.isShuttingDown()) return@post
                host.handleInboundFragment(address, data)
            }
        }
    }

    // --- Connection state handlers ---

    private fun handleConnected(gatt: BluetoothGatt) {
        val address = gatt.device.address
        Log.i(TAG, "GATT client: Connected to $address")
        diagnosticEmitter("info", "Connected to BLE device", mapOf("address" to address))
        // Socket is up, so any prior backoff for this address is obsolete.
        // Reset here (not on deviceId read) so a rebound link that races a
        // subsequent disconnect still benefits.
        connectionRetryCount.remove(address)
        linkReady.remove(address)
        try {
            Log.d(TAG, "Starting service discovery for $address")
            val discoveryStarted = gatt.discoverServices()
            Log.d(TAG, "Service discovery ${if (discoveryStarted) "started" else "FAILED"} for $address")
            diagnosticEmitter("debug", "Service discovery initiated", mapOf(
                "address" to address,
                "started" to discoveryStarted,
            ))
            if (!discoveryStarted) {
                // discoverServices returns false when the stack is busy or
                // the client is in a bad state. The async onServicesDiscovered
                // callback will never fire, so we close the client
                // synchronously.
                //
                // This is a permanent drop by design: closeGattClient removes
                // the address from connections immediately, so the
                // STATE_DISCONNECTED callback that follows will observe
                // `wasConnected = false` and skip the retry-with-backoff
                // path. A fresh advertisement (or a later scan rehydration)
                // will trigger a new connectToDevice cycle — we don't thrash
                // an already-sick stack in the meantime.
                diagnosticEmitter("warning", "discoverServices returned false; closing gatt", mapOf(
                    "address" to address,
                ))
                closeGattClient(gatt, "discover_services_false")
            }
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied discovering services", e)
            diagnosticEmitter("error", "Permission denied discovering services", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        } catch (e: Exception) {
            Log.e(TAG, "Error discovering services", e)
            diagnosticEmitter("error", "Error discovering services", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }
    }

    private fun handleDisconnected(gatt: BluetoothGatt, status: Int) {
        val address = gatt.device.address
        val wasConnected = host.connections.getGatt(address) != null
        host.connections.removeGatt(address)
        linkReady.remove(address)

        // Don't remove from discovered list - keep trying to reconnect.
        // Only clear cached RSSI on Android's catch-all 0x85 GATT_ERROR —
        // see [GATT_GENERIC_ERROR] for why this status is special.
        if (status == GATT_GENERIC_ERROR) {
            host.clearRssi(address)
        }

        if (wasConnected && host.isRunning()) {
            // Increment retry count and calculate backoff
            val retryCount = (connectionRetryCount[address] ?: 0) + 1
            connectionRetryCount[address] = retryCount

            if (retryCount > MAX_CONNECTION_RETRIES) {
                Log.w(TAG, "Max retries ($MAX_CONNECTION_RETRIES) exceeded for $address on disconnect, giving up")
                diagnosticEmitter("warning", "Max connection retries exceeded", mapOf(
                    "address" to address,
                    "retryCount" to retryCount,
                ))
                connectionRetryCount.remove(address)
                // Notify peer lost since we're giving up. Post the entire
                // body to the main handler so connection bookkeeping, queue
                // clear, and peer-loss notification land as one atomic step
                // against drainAndSendFragments — Handler posts are totally
                // ordered per-looper, so serialisation is preserved without
                // blocking this Bluetooth binder thread on a main-handler
                // latch.
                val peerId = host.connections.deviceIdForAddress(address)
                if (peerId != null) {
                    mainHandler.post { finalizeGivenUpPeer(address, peerId) }
                }
                return
            }

            // Exponential backoff: 5s, 10s, 20s, 40s, 60s (capped)
            val backoffInterval = minOf(
                MAX_RECONNECT_INTERVAL_MS,
                MIN_RECONNECT_INTERVAL_MS * (1L shl (retryCount - 1)),
            )

            mainHandler.postDelayed({
                if (host.isRunning() && host.connections.hasDeviceForAddress(address)) {
                    try {
                        val device = host.bluetoothAdapter?.getRemoteDevice(address)
                        if (device != null) {
                            host.connectToDevice(device)
                        }
                    } catch (e: Exception) {
                        Log.e(TAG, "Error reconnecting to device", e)
                    }
                }
            }, backoffInterval)
        } else {
            // Stale callback (wasConnected=false) or transport stopped
            // mid-flight (!isRunning). We have to notify the protocol of
            // peer loss, but the prior shape of this branch called UniFFI
            // (removeNeighborRoutes, blePeerLost) directly from the
            // Bluetooth binder thread. That is exactly the pattern the
            // rest of this file is built to avoid — it blocks GATT work
            // for every client in this process while the UniFFI mutex is
            // held. Post the whole body to main and reuse the
            // give-up-cleanup helper so the queue/state clear stays
            // consistent with the max-retries path.
            val peerId = host.connections.deviceIdForAddress(address)
            if (peerId != null) {
                mainHandler.post { finalizeGivenUpPeer(address, peerId) }
            }
        }
    }

    /**
     * Tear down every scrap of main-thread state held for a peer we are
     * giving up on — both the max-retries-exhausted branch and the
     * stale-callback/!isRunning branch funnel through here so the cleanup
     * sequence stays single-sourced.
     *
     * Must run on the main thread. Drops the outbound queue for this
     * peer alongside the inbound buffer — without that, fragments
     * destined for an address that has been permanently lost sit in the
     * outbound queue until either the 30s per-entry TTL or capacity
     * eviction fires, wasting memory and (worse) potentially delivering
     * stale fragments if the peer comes back on a new session.
     */
    private fun finalizeGivenUpPeer(address: String, peerId: String) {
        if (host.isShuttingDown()) return
        // Cancel any watchdog that may still be in flight for this
        // address — `closeGattClient` usually handles this earlier in
        // the teardown path, but the stale-callback branch in
        // `onConnectionStateChange` can reach here without having gone
        // through close first.
        cancelMtuWatchdog(address)
        host.protocol.removeNeighborRoutes(peerId)
        try {
            host.protocol.blePeerLost(peerId)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying peer lost", e)
            diagnosticEmitter("error", "Error notifying peer lost", mapOf(
                "exception" to e.javaClass.simpleName,
                "message" to (e.message ?: "unknown"),
            ))
        }
        host.onPeerGivenUp(address, peerId)
        host.meshController.registerDisconnection(peerId)
        host.refreshSelfMetrics()
        host.connections.removeIdentifiersForAddress(address)
        host.connections.removeConnectionRole(peerId)
        clearPeerBuffers(
            address = address,
            peerId = peerId,
            pendingInbound = host.pendingInbound,
            outboundQueue = host.outboundQueue,
            resolutionAttempts = deviceIdResolutionAttempts,
        )
        if (host.isRunning()) {
            host.refreshAdvertising("disconnect_give_up")
        }
        host.maybeHandleRebalance("disconnect_give_up")
    }

    // --- Read handlers (main-thread) ---

    private fun handleCharacteristicReadOnMain(
        gatt: BluetoothGatt,
        charUuid: UUID,
        value: ByteArray?,
        status: Int,
    ) {
        val address = gatt.device.address
        Log.i(TAG, "onCharacteristicRead (main): $address, char=$charUuid, status=$status")

        if (status != BluetoothGatt.GATT_SUCCESS) {
            Log.w(TAG, "Characteristic read failed for $address, char=$charUuid, status=$status")
            diagnosticEmitter("warning", "Characteristic read failed", mapOf(
                "address" to address,
                "char" to charUuid.toString(),
                "status" to status,
            ))
            // Both reads are load-bearing now. The identity read used to be
            // diagnostic — "the peer is still usable, just unverified" — but
            // an unverified peer is no longer usable: its advertised id is
            // never announced, so the link would sit open addressing nobody.
            // Close on either failure.
            if (charUuid == deviceIdCharUuid) {
                closeGattClient(gatt, "device_id_read_failed")
            } else if (charUuid == identityCharUuid) {
                advertisedDeviceIds.remove(address)
                closeGattClient(gatt, "identity_read_failed")
            }
            return
        }

        when (charUuid) {
            deviceIdCharUuid -> handleDeviceIdRead(gatt, value)
            identityCharUuid -> handleIdentityRead(gatt, value)
        }
    }

    private fun handleDeviceIdRead(gatt: BluetoothGatt, value: ByteArray?) {
        val address = gatt.device.address
        val deviceIdValue = value?.toString(Charsets.UTF_8)
        Log.i(TAG, "Read advertised device id from $address: $deviceIdValue")

        if (deviceIdValue.isNullOrEmpty()) {
            Log.w(TAG, "Empty or null device ID from $address")
            diagnosticEmitter("warning", "Empty device ID characteristic value", mapOf("address" to address))
            closeGattClient(gatt, "empty_device_id")
            return
        }

        // Record the claim and go get the proof. NOTHING is announced here:
        // the advertised string is unauthenticated until the identity read
        // binds it to a key, and everything that used to happen in this
        // function — the registry mapping, the mesh registration, the MTU
        // flush, `blePeerDiscovered`, the fragment drain — now happens in
        // [announceVerifiedPeer], after that binding holds.
        advertisedDeviceIds[address] = deviceIdValue

        // Kick the identity read. It is no longer "diagnostic": it is the
        // only thing that can make this peer real, so every path that fails
        // to obtain it now closes the link instead of proceeding to CCCD.
        // A peer we cannot bind to a key is one we will never address.
        val service = gatt.getService(serviceUuid)
        val identityChar = service?.getCharacteristic(identityCharUuid)
        if (identityChar == null) {
            Log.w(TAG, "Identity characteristic not found on $address; peer cannot prove its id")
            rejectPeer(gatt, PeerIdentityBinding.Reason.UNVERIFIED_IDENTITY, "identity characteristic missing")
            return
        }
        try {
            val started = gatt.readCharacteristic(identityChar)
            if (!started) {
                Log.w(TAG, "readCharacteristic(identity) returned false for $address")
                rejectPeer(gatt, PeerIdentityBinding.Reason.UNVERIFIED_IDENTITY, "identity read could not be started")
            }
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied reading identity characteristic", e)
            rejectPeer(gatt, PeerIdentityBinding.Reason.UNVERIFIED_IDENTITY, "identity read permission denied")
        }
    }

    private fun handleIdentityRead(gatt: BluetoothGatt, value: ByteArray?) {
        val address = gatt.device.address
        val derivedAddress = verifyIdentityAndDerive(value, address)
        val advertised = advertisedDeviceIds[address]

        when (val outcome = PeerIdentityBinding.resolve(advertised, derivedAddress)) {
            is PeerIdentityBinding.Outcome.Rejected -> {
                rejectPeer(gatt, outcome.reason, "advertised=$advertised derived=$derivedAddress")
                return
            }
            is PeerIdentityBinding.Outcome.Verified -> {
                announceVerifiedPeer(gatt, outcome.peerId)
                // Identity proved → now enable notifications.
                enableNotificationsOnLink(gatt)
            }
        }
    }

    /**
     * Publishes a peer that has proved the id it advertised.
     *
     * This is the only place `blePeerDiscovered` is reached from the central
     * role. Before it, a connected peer has no entry in the Rust `peers` map,
     * no MTU, no mesh registration and no route — it is connected but not
     * addressable, which is the correct state for a peer whose id is still a
     * claim.
     *
     * [peerId] is the *derived* address rather than the advertised string.
     * They are equal by construction here — [PeerIdentityBinding] refuses
     * anything else — but taking it from the proof means no future relaxation
     * of that comparison can put an unproven id into the registry, the MTU
     * map, or `Message.recipient`.
     */
    private fun announceVerifiedPeer(gatt: BluetoothGatt, peerId: String) {
        val address = gatt.device.address
        Log.i(TAG, "Verified peer $peerId, mapping $address -> $peerId")
        host.connections.setDeviceIdentifier(address, peerId)

        val role = host.connections.consumePendingRole(address) ?: MeshController.MeshRole.MEMBER
        host.meshController.registerConnection(peerId, role)
        host.connections.setConnectionRole(peerId, role)
        host.meshController.markPeerActive(peerId)
        host.meshController.markPeerActive(host.selfDeviceId)
        host.refreshSelfMetrics()
        // Already on main — no extra hop needed.
        if (host.isRunning()) {
            host.refreshAdvertising("membership_change")
        }
        val rssiInt = host.rssiFor(address)?.toInt()
        if (rssiInt != null) {
            host.meshController.updatePeerMetrics(
                peerId,
                MeshController.PeerMetrics(rssi = rssiInt),
            )
        }

        // ORDERING INVARIANT — DO NOT REORDER.
        //
        // `onDeviceIdResolved` (which flushes the staged MTU via
        // `BleTransportFacade.flushPeerMtu` → `bleSetPeerMtu`) MUST
        // run before `protocol.blePeerDiscovered` below. This is
        // the entire point of commit ac8cce8: any fragmenting send
        // that lands between announcing the peer to the protocol
        // layer and flushing the MTU into the Rust transport will
        // key-miss `peer_mtus`, fall back to the 185-byte floor,
        // and silently waste ~60% of the negotiated BLE 5
        // bandwidth for every fragment in that window. The Rust
        // side pins this from its end via
        // `test_ble_golden_path_handshake_never_falls_back` and
        // the `ble_fragment_fallback_count` telemetry counter,
        // which fires the moment a registered peer has to fall
        // back.
        //
        // The identity cross-check moved BOTH calls out of
        // `handleDeviceIdRead` and into this method, one GATT read later.
        // Their order relative to each other is unchanged and must stay
        // that way — the invariant is that no fragmenting send can observe
        // an announced peer with no MTU on file.
        //
        // If you are refactoring this function and think these two
        // calls should swap "for clarity", don't.
        host.onDeviceIdResolved(address, peerId)

        val rssi = host.rssiFor(address) ?: (-60).toShort()
        try {
            // ORDERING INVARIANT: this line MUST come AFTER
            // `host.onDeviceIdResolved` above.
            host.protocol.blePeerDiscovered(peerId, rssi)
        } catch (e: Exception) {
            Log.e(TAG, "Error notifying peer discovered", e)
            diagnosticEmitter("error", "Error notifying peer discovered", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
        }

        // Seed the route with the same verified address, now that the peer
        // exists to the protocol layer. This used to run inside the identity
        // handler ahead of the announce, which could learn a route to a peer
        // `peers` had no entry for.
        val quality = minOf(1.0f, maxOf(0.0f, (rssi.toFloat() + 100f) / 80f))
        try {
            host.protocol.learnRoute(peerId, peerId, 1.toUByte(), quality, 0u)
        } catch (e: Exception) {
            Log.e(TAG, "Error learning route for $peerId", e)
        }

        // Drain all pending inbound fragments keyed by this address — the
        // same buffer holds central-side notify fragments and server-side
        // write fragments, since both paths queue under the
        // connection-specific address.
        drainPendingInboundFor(address, peerId)
    }

    /**
     * Closes a link whose peer could not prove the id it advertised.
     *
     * Nothing needs retracting: no announce happens before
     * [announceVerifiedPeer], so a refused peer was never visible to the
     * protocol layer. The link is dropped rather than kept unannounced —
     * holding a connection we will never address costs a slot on both ends,
     * and closing it lets the ordinary reconnect path retry, which is what
     * recovers a peer that simply had not finished initializing its identity.
     */
    private fun rejectPeer(gatt: BluetoothGatt, reason: String, detail: String) {
        val address = gatt.device.address
        advertisedDeviceIds.remove(address)
        Log.w(TAG, "Refusing peer $address: $reason ($detail)")
        diagnosticEmitter(
            "warning",
            "BLE peer refused: unproven identity",
            mapOf("address" to address, "reason" to reason, "detail" to detail),
        )
        closeGattClient(gatt, reason)
    }

    private fun enableNotificationsOnLink(gatt: BluetoothGatt) {
        val address = gatt.device.address
        val service = gatt.getService(serviceUuid)
        val messageChar = service?.getCharacteristic(messageCharUuid)
        if (messageChar == null) {
            Log.w(TAG, "Message characteristic NOT FOUND on $address")
            diagnosticEmitter("warning", "Message characteristic missing", mapOf("address" to address))
            closeGattClient(gatt, "message_char_missing")
            return
        }
        Log.d(TAG, "Enabling notifications for message characteristic on $address")
        try {
            val notifyEnabled = gatt.setCharacteristicNotification(messageChar, true)
            if (!notifyEnabled) {
                Log.w(TAG, "Local notification sink FAILED for $address")
                diagnosticEmitter("warning", "setCharacteristicNotification returned false", mapOf("address" to address))
                closeGattClient(gatt, "set_notification_failed")
                return
            }
            val cccd = messageChar.getDescriptor(PeripheralGattServer.CCCD_UUID)
            if (cccd == null) {
                Log.w(TAG, "CCCD descriptor missing on remote message characteristic for $address")
                diagnosticEmitter("warning", "Remote CCCD descriptor missing", mapOf("address" to address))
                closeGattClient(gatt, "remote_cccd_missing")
                return
            }
            val cccdWriteOk = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                val cccdWriteStatus = gatt.writeDescriptor(
                    cccd,
                    BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE,
                )
                Log.d(TAG, "CCCD write status=$cccdWriteStatus for $address")
                cccdWriteStatus == BluetoothStatusCodes.SUCCESS
            } else {
                @Suppress("DEPRECATION")
                cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE
                @Suppress("DEPRECATION")
                val queued = gatt.writeDescriptor(cccd)
                Log.d(TAG, "CCCD write ${if (queued) "queued" else "FAILED"} for $address")
                queued
            }
            if (!cccdWriteOk) {
                diagnosticEmitter("warning", "CCCD writeDescriptor failed to initiate", mapOf("address" to address))
                closeGattClient(gatt, "cccd_write_rejected")
            }
            // Success path waits for onDescriptorWrite to fire.
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied setting notification", e)
            diagnosticEmitter("error", "Permission denied enabling notifications", mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")))
            closeGattClient(gatt, "set_notification_permission_denied")
        }
    }

    private fun closeGattClient(gatt: BluetoothGatt, reason: String) {
        val address = gatt.device.address
        try {
            gatt.disconnect()
            gatt.close()
        } catch (e: Exception) {
            Log.w(TAG, "Error closing gatt for $address ($reason)", e)
        }
        host.connections.removeGatt(address)
        linkReady.remove(address)
        // Drop the unproven claim with the link, so a reconnect cannot pair
        // this connection's device id with the next one's identity.
        advertisedDeviceIds.remove(address)
        cancelMtuWatchdog(address)
    }

    /**
     * Verifies a signed identity blob read from a peer and returns the address
     * its key derives to, or null if the peer proved nothing.
     *
     * Null covers every failure alike — absent, undecodable, bad signature,
     * underivable key — because they are the same thing to the caller: no
     * proof. [PeerIdentityBinding] turns that into a refusal; the specific
     * cause is carried in the diagnostics rather than in the return type.
     *
     * The signature is over the peer's 19-byte mesh advertisement, so what it
     * establishes is possession of the private key for `publicKey` — which is
     * exactly what makes the derivation meaningful. Deriving without checking
     * the signature would prove nothing at all: anyone can copy a public key.
     */
    private fun verifyIdentityAndDerive(data: ByteArray?, address: String): String? {
        val signedIdentity = SignedIdentityData.decode(data)
        if (signedIdentity == null) {
            Log.w(TAG, "Failed to decode identity data from $address")
            diagnosticEmitter("warning", "Failed to decode peer identity", mapOf("address" to address))
            return null
        }

        return try {
            val isValid = host.protocol.verifySignature(
                signedIdentity.publicKey.map { it.toUByte() },
                signedIdentity.advertisementData.map { it.toUByte() },
                signedIdentity.signature.map { it.toUByte() },
            )

            if (isValid) {
                // Derived in Rust — the single implementation shared by every
                // platform. `decode` already guarantees a 32-byte key, so this
                // cannot throw here; a throw would land in the catch below.
                val derivedAddress = deriveAddress(signedIdentity.publicKey.map { it.toUByte() })
                Log.i(TAG, "Verified peer identity: $derivedAddress for $address")
                diagnosticEmitter("info", "Verified peer identity", mapOf(
                    "address" to address,
                    "derivedAddress" to derivedAddress,
                ))
                derivedAddress
            } else {
                Log.w(TAG, "Invalid signature for peer $address")
                diagnosticEmitter("warning", "Invalid peer signature", mapOf("address" to address))
                null
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to verify signature: ${e.message}", e)
            diagnosticEmitter("error", "Signature verification failed", mapOf("error" to (e.message ?: "unknown")))
            null
        }
    }

    /**
     * Drain any inbound fragments that had been buffered while waiting for
     * [address]'s device id to resolve. Forwards each fragment to the
     * protocol in FIFO order and learns routes from any complete messages
     * that fall out of the reassembler.
     *
     * Called from [handleDeviceIdRead], which runs on the main thread, so
     * all protocol calls here are safely off the binder thread.
     */
    private fun drainPendingInboundFor(address: String, deviceId: String) {
        clearResolutionAttempt(address)
        var iterations = 0
        while (iterations < MAX_DRAIN_ITERATIONS) {
            val fragments = host.pendingInbound.drain(address)
            if (fragments.isEmpty()) break
            iterations++
            for (fragment in fragments) {
                try {
                    val bytes = fragment.map { it.toUByte() }
                    host.protocol.bleFragmentReceived(deviceId, bytes)

                    var msg = host.protocol.receiveMessage()
                    while (msg != null) {
                        Log.i(TAG, "Complete message assembled from queued fragments for $deviceId")
                        diagnosticEmitter("info", "Complete message assembled from queued fragments", mapOf(
                            "senderId" to deviceId,
                            "messageContent" to msg,
                        ))
                        host.learnRouteFromMessage(msg, deviceId, address)
                        msg = host.protocol.receiveMessage()
                    }
                } catch (e: Exception) {
                    Log.e(TAG, "Error processing pending fragment", e)
                }
            }
        }
        if (iterations >= MAX_DRAIN_ITERATIONS) {
            Log.w(TAG, "drainPendingInboundFor hit drain iteration cap for $address")
            diagnosticEmitter(
                "warning",
                "Pending-fragment drain iteration cap reached",
                mapOf("address" to address, "deviceId" to deviceId),
            )
        }
    }

}

/**
 * Drop every piece of main-thread-only buffer state the central client
 * holds for a peer we are giving up on — the inbound fragment buffer
 * keyed by BLE address, the outbound fragment queue keyed by device id,
 * and the device-id resolution throttle map.
 *
 * Extracted as a file-level helper so it can be exercised directly in a
 * plain JVM test. Before this helper existed, the give-up path cleared
 * `pendingInbound` but forgot to clear `outboundQueue`, causing fragments
 * destined for a peer we had just abandoned to sit in the queue until the
 * per-entry 30s TTL or a capacity eviction fired — wasting memory and
 * risking stale delivery if the peer came back on a fresh session. The
 * test that guards this regression lives in `CentralGattClientTest`.
 */
internal fun clearPeerBuffers(
    address: String,
    peerId: String,
    pendingInbound: InboundFragmentBuffer,
    outboundQueue: OutboundFragmentQueue,
    resolutionAttempts: MutableMap<String, Long>,
) {
    pendingInbound.removeAll(address)
    outboundQueue.removeAll(peerId)
    resolutionAttempts.remove(address)
}
