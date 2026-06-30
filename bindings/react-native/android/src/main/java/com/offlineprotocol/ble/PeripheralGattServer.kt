package com.offlineprotocol.ble

import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothStatusCodes
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * Return the portion of [value] that should be sent in response to a GATT read
 * at [offset]. Android's long-read flow issues repeated
 * `onCharacteristicReadRequest` calls with increasing offset until the central
 * has reassembled the whole value; the app must return the tail starting at
 * that offset on each call. Exposed internal for unit testing.
 */
internal fun sliceForReadOffset(value: ByteArray, offset: Int): ByteArray = when {
    offset <= 0 -> value
    offset >= value.size -> ByteArray(0)
    else -> value.copyOfRange(offset, value.size)
}

/**
 * Outcome of a CCCD (0x2902) descriptor write, per the spec:
 *
 *   - `0x0001` little-endian → enable notifications
 *   - `0x0002` little-endian → enable indications
 *   - anything else → disable (including the explicit `0x0000`)
 *
 * Both `enable` cases collapse to [ENABLE] because the facade treats
 * subscribed-central bookkeeping identically for notify and indicate
 * delivery modes.
 */
internal enum class CccdAction { ENABLE, DISABLE }

// Duplicated from [BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE] /
// ENABLE_INDICATION_VALUE so the classifier stays a pure function that
// can be exercised in plain JVM unit tests. The Android framework stub
// jar throws on static-field access under the Gradle `test` task, so
// referencing those constants directly would take this helper out of
// reach of the existing test harness.
internal val CCCD_ENABLE_NOTIFICATION_BYTES: ByteArray = byteArrayOf(0x01, 0x00)
internal val CCCD_ENABLE_INDICATION_BYTES: ByteArray = byteArrayOf(0x02, 0x00)

/**
 * Classify a CCCD descriptor write value as a subscribe or unsubscribe.
 * Unknown / malformed payloads are treated as [CccdAction.DISABLE] so a
 * garbage write can only ever narrow the subscriber set, never widen it.
 */
internal fun classifyCccdWrite(value: ByteArray): CccdAction =
    if (value.contentEquals(CCCD_ENABLE_NOTIFICATION_BYTES) ||
        value.contentEquals(CCCD_ENABLE_INDICATION_BYTES)
    ) {
        CccdAction.ENABLE
    } else {
        CccdAction.DISABLE
    }

/**
 * Owns the Android peripheral-side GATT server. Wraps the raw
 * [BluetoothGattServer] and enforces:
 *
 *   1. Every notify-propertied characteristic automatically gets a CCCD
 *      (0x2902) descriptor via [registerNotifyCharacteristic], so remote
 *      centrals can subscribe to notifications. The helper is the only
 *      sanctioned way to create notify characteristics on this server.
 *   2. [BluetoothGattServerCallback.onDescriptorWriteRequest] is implemented
 *      so CCCD writes from centrals are acked and a count of subscribed
 *      centrals is maintained for observability.
 *   3. Service registration has a 5-second watchdog: if `onServiceAdded`
 *      never fires, we tear down, retry once, and escalate to
 *      [Listener.onSetupFailed] on a second failure — so a flaky GATT stack
 *      cannot silently leave the device invisible on the mesh.
 *   4. `onCharacteristicReadRequest` honours the read offset, so long-read
 *      flows (identity payloads that exceed the default 20-byte ATT MTU
 *      payload) return the correct tail on each follow-up read.
 *   5. `onCharacteristicWriteRequest` enforces a hard upper bound on inbound
 *      payload size as defence in depth against malformed/hostile centrals.
 *
 * Note: the facade currently routes outbound fragments through client-side
 * `writeCharacteristic` calls, not server-side notifications. The CCCD
 * bookkeeping here is kept for the day when asymmetric mesh links (we are
 * only the peripheral for a given peer) need a notify fallback — until
 * then it is purely diagnostic.
 */
class PeripheralGattServer(
    private val context: Context,
    private val mainHandler: Handler,
    private val listener: Listener,
    private val diagnosticEmitter: (level: String, message: String, context: Map<String, Any?>) -> Unit =
        { _, _, _ -> }
) {

    /** Callbacks surfaced to the orchestrating transport. */
    interface Listener {
        fun onCentralConnected(device: BluetoothDevice)
        fun onCentralDisconnected(device: BluetoothDevice, status: Int)
        fun onInboundFragment(device: BluetoothDevice, bytes: ByteArray)
        /** A notification we pushed to [device] (via [notifyFragment]) has been
         *  flushed by the stack — the next notification to that central may now
         *  go. Lets the transport release its per-peer send gate on real
         *  completion instead of relying solely on the watchdog timeout. */
        fun onNotificationSent(device: BluetoothDevice, status: Int)
        /** The ATT MTU for [device]'s connection to OUR GATT server has been
         *  (re)negotiated by that central. [maxPayload] is the header-adjusted
         *  max NOTIFY payload (MTU − 3) we can push to it via [notifyFragment].
         *  The transport bounds the per-peer fragment size by this so a message
         *  sized for OUR central link to the peer is not over-sized for this
         *  notify link — the cause of the offline 1:1 Welcome-delivery stall. */
        fun onPeripheralMtuNegotiated(device: BluetoothDevice, maxPayload: Int)
        /** Return the current device-ID bytes, or null to fail the read. */
        fun provideDeviceIdBytes(device: BluetoothDevice): ByteArray?
        /** Return the current signed-identity bytes, or null to fail the read. */
        fun provideIdentityBytes(device: BluetoothDevice): ByteArray?
        fun onReady()
        fun onSetupFailed(reason: String)
    }

    companion object {
        private const val TAG = "PeripheralGattServer"
        val CCCD_UUID: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
        private const val SERVICE_READY_TIMEOUT_MS = 5_000L
        private const val RETRY_DELAY_MS = 500L
        private const val MAX_SETUP_ATTEMPTS = 2
        /** Hard ceiling on an inbound GATT write payload. Well above the
         *  facade's MAX_FRAGMENT_SIZE (185 bytes) to tolerate MTU-negotiation
         *  headroom, but bounded so a hostile central cannot balloon memory. */
        const val MAX_INBOUND_WRITE_BYTES = 4096
        /** ATT protocol header subtracted from a negotiated ATT MTU to get the
         *  usable NOTIFY payload (MTU − 3). Matches CentralGattClient's
         *  ATT_HEADER_BYTES so both link directions report payloads on the
         *  same basis. */
        private const val ATT_HEADER_BYTES = 3
        /** Maximum age of a per-central identity long-read snapshot. A
         *  coherent long-read session finishes in well under a second on
         *  real hardware; anything older is either the result of a central
         *  that disconnected without a clean STATE_DISCONNECTED (leaving
         *  the snapshot pinned) or a misbehaving peer we do not want to
         *  hold memory for. */
        private const val IDENTITY_SNAPSHOT_TTL_MS = 5_000L
    }

    private val bluetoothManager: BluetoothManager =
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager

    // @Volatile because these references are mutated on the main thread
    // (attemptSetup / handleSetupFailed / stop) but read on the GATT server's
    // binder thread in every callback. Without the visibility barrier a
    // binder callback can observe a stale non-null server during teardown and
    // operate on a closed instance.
    @Volatile
    private var gattServer: BluetoothGattServer? = null
    @Volatile
    private var messageCharacteristic: BluetoothGattCharacteristic? = null
    @Volatile
    private var deviceIdCharacteristic: BluetoothGattCharacteristic? = null
    @Volatile
    private var identityCharacteristic: BluetoothGattCharacteristic? = null

    private data class IdentitySnapshot(val bytes: ByteArray, val timestamp: Long)

    /**
     * Per-central snapshot of the identity bytes served on a long read.
     * Android's long-read flow issues repeated reads with increasing offsets
     * until the central has reassembled the whole value; every slice in one
     * reassembly has to come from a *single* point-in-time snapshot or the
     * central's signature check will fail on a torn buffer.
     *
     * Populated on `offset == 0` (the start of a read session) and served on
     * every follow-up offset for the same central. Cleared when the central
     * disconnects, when the server is stopped, when a fresh `offset == 0`
     * read replaces it, or when the snapshot exceeds [IDENTITY_SNAPSHOT_TTL_MS]
     * — the TTL bounds the damage from a central that starts a long-read
     * session and never issues the follow-up reads to complete it.
     */
    private val identityReadSnapshots: ConcurrentHashMap<String, IdentitySnapshot> =
        ConcurrentHashMap()

    @Volatile
    var isReady: Boolean = false
        private set

    /**
     * Addresses of centrals that have written ENABLE_NOTIFICATION_VALUE to
     * the CCCD. Kept purely for observability — the facade currently routes
     * outbound fragments through client-side writes, never server-side
     * notifications, so nothing reads this set beyond the diagnostic
     * emissions in [onDescriptorWriteRequest]. Keyed by address (not
     * [BluetoothDevice]) so the contract matches [MeshConnectionRegistry]'s
     * stated "stable for the lifetime of a single LL connection" guarantee
     * instead of relying on [BluetoothDevice.equals] delegating to the
     * address string.
     */
    private val subscribedCentralAddresses: MutableSet<String> =
        ConcurrentHashMap.newKeySet()

    private var serviceReadyTimeoutRunnable: Runnable? = null
    private var setupRetryRunnable: Runnable? = null
    private var setupAttempts = 0
    private var pendingSetup: PendingSetup? = null

    private data class PendingSetup(
        val serviceUuid: UUID,
        val messageUuid: UUID,
        val deviceIdUuid: UUID,
        val identityUuid: UUID,
    )

    /**
     * Open the GATT server and register the mesh service. Idempotent: a
     * second call while already started tears down and re-attempts setup.
     * Must be called from the main thread — the facade posts start/stop
     * through its `runOnMainSync` helper and every callback we fire runs
     * on the main handler, so the internal state machine assumes no
     * concurrent entry.
     */
    fun start(
        serviceUuid: UUID,
        messageUuid: UUID,
        deviceIdUuid: UUID,
        identityUuid: UUID,
    ) {
        check(Looper.myLooper() == Looper.getMainLooper()) {
            "PeripheralGattServer.start must be called on the main thread"
        }
        stop()
        setupAttempts = 0
        pendingSetup = PendingSetup(serviceUuid, messageUuid, deviceIdUuid, identityUuid)
        attemptSetup()
    }

    /** Close the server and drop all cached state. Safe to call repeatedly. */
    fun stop() {
        check(Looper.myLooper() == Looper.getMainLooper()) {
            "PeripheralGattServer.stop must be called on the main thread"
        }
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        serviceReadyTimeoutRunnable = null
        setupRetryRunnable?.let { mainHandler.removeCallbacks(it) }
        setupRetryRunnable = null
        try {
            gattServer?.close()
        } catch (e: SecurityException) {
            Log.w(TAG, "Permission denied while closing GATT server", e)
        }
        gattServer = null
        messageCharacteristic = null
        deviceIdCharacteristic = null
        identityCharacteristic = null
        subscribedCentralAddresses.clear()
        identityReadSnapshots.clear()
        isReady = false
        pendingSetup = null
        setupAttempts = 0
    }

    /** Count of centrals currently subscribed via CCCD. Observability only. */
    fun subscribedCentralCount(): Int = subscribedCentralAddresses.size

    /** True if [address] is a central currently subscribed to the message
     *  characteristic via CCCD — i.e. we can push notifications to it over the
     *  connection it opened to our GATT server. [subscribedCentralAddresses] is
     *  a concurrent set so this is safe to read from the main thread. */
    fun isSubscribed(address: String): Boolean =
        subscribedCentralAddresses.contains(address)

    /** Snapshot of the addresses of centrals currently subscribed via CCCD.
     *  Used to resolve the notify egress by PEER IDENTITY when the address we
     *  hold for a peer (the central link we opened to it) differs from the
     *  address it subscribed under (the link it opened to our server). */
    fun subscribedAddresses(): Set<String> = subscribedCentralAddresses.toSet()

    /**
     * Reply to a central by notifying the message characteristic over the
     * connection IT opened to our GATT server. This is the egress for
     * asymmetric mesh links where we are only the peripheral for [device] (see
     * the class note above): the central's reverse link back to us may be
     * absent or half-open, so a central-role `writeCharacteristic(NO_RESPONSE)`
     * returns SUCCESS into a dead link and the bytes are silently dropped on air
     * (no status 201, no error) — the peer never gets the reply and retransmits
     * forever. Notifying over the live, subscribed connection is reliable.
     *
     * Returns true if the stack accepted the notification. No-op (false) if the
     * server/characteristic is gone or [device] is not subscribed. Notifications
     * are serialised one-outstanding-per-connection by the stack, just like
     * NO_RESPONSE writes, so the caller paces them via the shared write gate.
     */
    fun notifyFragment(device: BluetoothDevice, bytes: ByteArray): Boolean {
        val server = gattServer ?: return false
        val characteristic = messageCharacteristic ?: return false
        if (!subscribedCentralAddresses.contains(device.address)) return false
        return try {
            // confirm = true → ATT Handle Value INDICATION (acknowledged) rather
            // than a fire-and-forget NOTIFICATION. The central must confirm each
            // one before the stack accepts the next, giving the per-fragment flow
            // control that keeps a large Welcome from overrunning the receiver.
            // onNotificationSent then fires on the CONFIRMATION (real delivery),
            // which releases the per-peer write gate only once the peer has it.
            val accepted = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                server.notifyCharacteristicChanged(device, characteristic, true, bytes) ==
                    BluetoothStatusCodes.SUCCESS
            } else {
                @Suppress("DEPRECATION")
                characteristic.value = bytes
                @Suppress("DEPRECATION")
                server.notifyCharacteristicChanged(device, characteristic, true)
            }
            if (!accepted) {
                // The stack refused the notification. The dominant cause for a
                // multi-fragment payload is a fragment larger than this notify
                // link's ATT_MTU − 3. Surface the size so a stalled Welcome is
                // attributable to an MTU mismatch rather than silent loss.
                diagnosticEmitter(
                    "warning",
                    "notify rejected by stack",
                    mapOf("address" to device.address, "fragmentSize" to bytes.size),
                )
            }
            accepted
        } catch (e: Exception) {
            Log.e(TAG, "notifyFragment failed for ${device.address}", e)
            false
        }
    }

    /** Cancel an in-flight connection to a central. */
    fun cancelConnection(device: BluetoothDevice) {
        try {
            gattServer?.cancelConnection(device)
        } catch (e: SecurityException) {
            Log.w(TAG, "Permission denied cancelling connection to ${device.address}", e)
        }
    }

    // --- Internal setup ---

    private fun attemptSetup() {
        val setup = pendingSetup ?: return
        setupAttempts++
        isReady = false

        val server = try {
            bluetoothManager.openGattServer(context, gattServerCallback)
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied opening GATT server", e)
            diagnosticEmitter(
                "error",
                "gatt_server_permission_denied",
                mapOf("exception" to e.javaClass.simpleName, "message" to (e.message ?: "unknown")),
            )
            listener.onSetupFailed("permission_denied: ${e.message}")
            return
        }
        if (server == null) {
            Log.e(TAG, "openGattServer returned null")
            handleSetupFailed("open_gatt_server_null")
            return
        }
        gattServer = server

        val message = registerNotifyCharacteristic(setup.messageUuid)
        val deviceIdChar = BluetoothGattCharacteristic(
            setup.deviceIdUuid,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        val identityChar = BluetoothGattCharacteristic(
            setup.identityUuid,
            BluetoothGattCharacteristic.PROPERTY_READ,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        messageCharacteristic = message
        deviceIdCharacteristic = deviceIdChar
        identityCharacteristic = identityChar

        val service = BluetoothGattService(
            setup.serviceUuid,
            BluetoothGattService.SERVICE_TYPE_PRIMARY,
        )
        service.addCharacteristic(message)
        service.addCharacteristic(deviceIdChar)
        service.addCharacteristic(identityChar)

        val added = try {
            server.addService(service)
        } catch (e: SecurityException) {
            Log.e(TAG, "Permission denied adding service", e)
            handleSetupFailed("permission_denied_add_service")
            return
        }
        if (!added) {
            handleSetupFailed("add_service_returned_false")
            return
        }

        armServiceReadyTimeout()
    }

    private fun armServiceReadyTimeout() {
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        val runnable = Runnable {
            if (!isReady) {
                Log.w(TAG, "GATT service ready timeout (attempt=$setupAttempts)")
                diagnosticEmitter(
                    "warn",
                    "gatt_service_ready_timeout",
                    mapOf("attempt" to setupAttempts),
                )
                handleSetupFailed("service_ready_timeout")
            }
        }
        serviceReadyTimeoutRunnable = runnable
        mainHandler.postDelayed(runnable, SERVICE_READY_TIMEOUT_MS)
    }

    private fun handleSetupFailed(reason: String) {
        serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
        serviceReadyTimeoutRunnable = null
        setupRetryRunnable?.let { mainHandler.removeCallbacks(it) }
        setupRetryRunnable = null
        try {
            gattServer?.close()
        } catch (_: SecurityException) {
            // Best effort — we're tearing down anyway.
        }
        gattServer = null
        messageCharacteristic = null
        deviceIdCharacteristic = null
        identityCharacteristic = null
        isReady = false

        if (setupAttempts >= MAX_SETUP_ATTEMPTS) {
            diagnosticEmitter(
                "error",
                "gatt_service_ready_timeout_final",
                mapOf("attempts" to setupAttempts, "reason" to reason),
            )
            val setup = pendingSetup
            pendingSetup = null
            setupAttempts = 0
            if (setup != null) {
                listener.onSetupFailed(reason)
            }
            return
        }

        val retry = Runnable {
            setupRetryRunnable = null
            if (pendingSetup != null) {
                attemptSetup()
            }
        }
        setupRetryRunnable = retry
        mainHandler.postDelayed(retry, RETRY_DELAY_MS)
    }

    /**
     * The only sanctioned path for creating a notify-propertied
     * characteristic on this server. Attaches a CCCD (0x2902) descriptor
     * unconditionally so remote centrals can subscribe via the standard
     * GATT notification subscription flow.
     */
    private fun registerNotifyCharacteristic(uuid: UUID): BluetoothGattCharacteristic {
        val char = BluetoothGattCharacteristic(
            uuid,
            // INDICATE, not NOTIFY: indications are ATT-confirmed, so the stack
            // will not send the next fragment until the central confirms the
            // previous one. That per-fragment flow control is what an
            // unacknowledged NOTIFY lacks — a large multi-fragment message (an
            // MLS Welcome) out-runs a slower central's receive buffer and loses
            // a fragment every pass, so it never reassembles. With only
            // INDICATE advertised, iOS subscribes for indications (it prefers
            // NOTIFY when both are offered), and the CCCD classifier already
            // accepts the 0x0002 enable value.
            BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or
                BluetoothGattCharacteristic.PROPERTY_INDICATE,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        val cccd = BluetoothGattDescriptor(
            CCCD_UUID,
            BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
        )
        char.addDescriptor(cccd)
        return char
    }

    // --- GattServerCallback ---

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(status: Int, service: BluetoothGattService?) {
            if (status == BluetoothGatt.GATT_SUCCESS) {
                Log.i(TAG, "GATT service added: ${service?.uuid}")
                diagnosticEmitter(
                    "info",
                    "gatt_service_registered",
                    mapOf("serviceUUID" to (service?.uuid?.toString() ?: "unknown")),
                )
                isReady = true
                serviceReadyTimeoutRunnable?.let { mainHandler.removeCallbacks(it) }
                serviceReadyTimeoutRunnable = null
                setupAttempts = 0
                listener.onReady()
            } else {
                Log.e(TAG, "GATT service add failed: status=$status")
                diagnosticEmitter(
                    "error",
                    "gatt_service_add_failed",
                    mapOf(
                        "status" to status,
                        "serviceUUID" to (service?.uuid?.toString() ?: "unknown"),
                    ),
                )
                isReady = false
                // Kick retry/escalation immediately rather than waiting out
                // the 5s watchdog — we already know the registration failed,
                // so there's no point stalling app startup.
                handleSetupFailed("service_add_status=$status")
            }
        }

        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> listener.onCentralConnected(device)
                BluetoothProfile.STATE_DISCONNECTED -> {
                    subscribedCentralAddresses.remove(device.address)
                    identityReadSnapshots.remove(device.address)
                    listener.onCentralDisconnected(device, status)
                }
            }
        }

        override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
            // A remote central (re)negotiated the ATT MTU on the connection IT
            // opened to OUR GATT server. This callback is the ONLY place the
            // peripheral/NOTIFY link's capacity is observable on the server
            // side — without it the transport keeps sizing notifications for
            // our central link to the peer and overflows this one, silently
            // truncating/dropping the multi-fragment MLS Welcome on air.
            // Surface the raw value as a diagnostic and the header-adjusted
            // payload to the transport, which folds it into the per-peer MTU.
            val maxPayload = (mtu - ATT_HEADER_BYTES).coerceAtLeast(0)
            diagnosticEmitter(
                "info",
                "peripheral_link_mtu",
                mapOf("address" to device.address, "mtu" to mtu, "maxPayload" to maxPayload),
            )
            listener.onPeripheralMtuNegotiated(device, maxPayload)
        }

        override fun onCharacteristicReadRequest(
            device: BluetoothDevice,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic,
        ) {
            val server = gattServer ?: return
            // Snapshot the nullable fields into locals so a concurrent
            // teardown on the main thread can't flip them between the match
            // and the listener call. Comparing against the stored
            // characteristic's UUID (rather than `nullable?.uuid`) also
            // removes the footgun where a `null == null` branch could match
            // an unrelated characteristic if both fields were ever cleared.
            val deviceIdChar = deviceIdCharacteristic
            val identityChar = identityCharacteristic
            val charUuid = characteristic.uuid

            val value: ByteArray? = try {
                when {
                    deviceIdChar != null && charUuid == deviceIdChar.uuid ->
                        listener.provideDeviceIdBytes(device)
                    identityChar != null && charUuid == identityChar.uuid ->
                        resolveIdentityValue(device, offset)
                    else -> null
                }
            } catch (e: Exception) {
                // A listener throwing is a programmer error, but we still
                // need to release the central from the pending read —
                // otherwise it sits spinning until the ATT timeout. Fall
                // through to the failure-response branch below.
                Log.w(TAG, "Listener threw in provide* callback", e)
                null
            }
            try {
                if (value != null) {
                    // Honour offset for long reads: the central will issue
                    // repeated reads with increasing offsets until it has
                    // reassembled a payload larger than the ATT MTU.
                    val slice = sliceForReadOffset(value, offset)
                    server.sendResponse(device, requestId, BluetoothGatt.GATT_SUCCESS, offset, slice)
                } else {
                    server.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in read request", e)
                // Best-effort: try to unblock the central with a failure
                // response. If we've genuinely lost BLUETOOTH_CONNECT this
                // will throw again and we just have to let the ATT timeout
                // take over.
                try {
                    server.sendResponse(device, requestId, BluetoothGatt.GATT_FAILURE, offset, null)
                } catch (_: SecurityException) {
                    // Permission already denied above; nothing more we can do.
                }
            }
        }

        /**
         * Serve the identity characteristic while keeping a long-read session
         * coherent. On `offset == 0` we fetch fresh bytes from the listener
         * and snapshot them per-central; on follow-up blob reads we serve
         * from the snapshot so the central reassembles bytes that all came
         * from the same point in time. Without this, a refresh of the
         * signed-identity cache mid-read lets the central stitch together a
         * torn value whose signature will fail to verify.
         *
         * Follow-up reads (`offset > 0`) without a matching snapshot fail
         * with `null`. That is deliberate: serving fresh bytes into the
         * middle of a long-read session stitches two different identity
         * values together and the central silently fails signature
         * verification on the reassembled buffer. Returning null forces the
         * central to restart the long-read at `offset == 0`, which gives
         * the next attempt a coherent snapshot to read from.
         */
        private fun resolveIdentityValue(device: BluetoothDevice, offset: Int): ByteArray? {
            val address = device.address
            val now = System.currentTimeMillis()
            pruneStaleIdentitySnapshots(now)
            if (offset == 0) {
                val fresh = listener.provideIdentityBytes(device)
                if (fresh != null) {
                    identityReadSnapshots[address] = IdentitySnapshot(fresh, now)
                } else {
                    identityReadSnapshots.remove(address)
                }
                return fresh
            }
            // Follow-up blob read within the same long-read session: must
            // serve from the snapshot taken on offset == 0.
            val cached = identityReadSnapshots[address]
            if (cached != null && now - cached.timestamp <= IDENTITY_SNAPSHOT_TTL_MS) {
                return cached.bytes
            }
            // No valid snapshot. Evict any stale entry and fail the read so
            // the central restarts the long-read cleanly on its next attempt.
            if (cached != null) {
                identityReadSnapshots.remove(address)
                diagnosticEmitter(
                    "warn",
                    "identity_long_read_snapshot_stale",
                    mapOf("address" to address, "offset" to offset),
                )
            } else {
                diagnosticEmitter(
                    "warn",
                    "identity_long_read_missing_snapshot",
                    mapOf("address" to address, "offset" to offset),
                )
            }
            return null
        }

        private fun pruneStaleIdentitySnapshots(now: Long) {
            if (identityReadSnapshots.isEmpty()) return
            val iterator = identityReadSnapshots.entries.iterator()
            while (iterator.hasNext()) {
                if (now - iterator.next().value.timestamp > IDENTITY_SNAPSHOT_TTL_MS) {
                    iterator.remove()
                }
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            listener.onNotificationSent(device, status)
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            val server = gattServer ?: return
            try {
                if (characteristic.uuid == messageCharacteristic?.uuid) {
                    // Defence in depth: drop oversized writes before handing
                    // the payload to the upstream fragment assembler.
                    if (value.size > MAX_INBOUND_WRITE_BYTES) {
                        diagnosticEmitter(
                            "warn",
                            "gatt_inbound_write_oversize",
                            mapOf(
                                "address" to device.address,
                                "size" to value.size,
                                "max" to MAX_INBOUND_WRITE_BYTES,
                            ),
                        )
                        if (responseNeeded) {
                            server.sendResponse(
                                device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                            )
                        }
                        return
                    }
                    listener.onInboundFragment(device, value)
                    if (responseNeeded) {
                        // Response value is semantically ignored by centrals;
                        // don't waste ATT bandwidth echoing the payload.
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null,
                        )
                    }
                } else {
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                        )
                    }
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in write request", e)
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray,
        ) {
            val server = gattServer ?: return
            try {
                if (descriptor.uuid == CCCD_UUID) {
                    when (classifyCccdWrite(value)) {
                        CccdAction.ENABLE -> {
                            subscribedCentralAddresses.add(device.address)
                            diagnosticEmitter(
                                "info",
                                "cccd_subscribe_received",
                                mapOf(
                                    "address" to device.address,
                                    "subscribedCount" to subscribedCentralAddresses.size,
                                ),
                            )
                        }
                        CccdAction.DISABLE -> {
                            subscribedCentralAddresses.remove(device.address)
                            diagnosticEmitter(
                                "info",
                                "cccd_unsubscribe_received",
                                mapOf(
                                    "address" to device.address,
                                    "subscribedCount" to subscribedCentralAddresses.size,
                                ),
                            )
                        }
                    }
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_SUCCESS, offset, null,
                        )
                    }
                } else {
                    if (responseNeeded) {
                        server.sendResponse(
                            device, requestId, BluetoothGatt.GATT_FAILURE, offset, null,
                        )
                    }
                }
            } catch (e: SecurityException) {
                Log.w(TAG, "Permission denied in descriptor write request", e)
            }
        }
    }
}
