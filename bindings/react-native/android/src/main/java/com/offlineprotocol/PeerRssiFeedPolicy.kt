package com.offlineprotocol

import android.bluetooth.le.AdvertiseData
import android.os.ParcelUuid
import java.util.UUID

/**
 * Which scan results feed a signal reading to the core, and what the scan
 * response carries for a scanner to attribute readings to this device.
 * The feed half mirrors ios/PeerRssiFeedPolicy.swift.
 *
 * Every failure here is silent: a throttle that never admits, or a hash that
 * never goes on air, looks exactly like no peer in range.
 */
internal object PeerRssiFeedPolicy {
    /**
     * A little over the core's 250 ms event cap. The two are measured on
     * different clocks with a lock wait between them, so an equal interval
     * let the core see a gap just under 250 ms, drop the reading, and halve
     * the rate.
     */
    const val INTERVAL_MS = 260L

    private const val MAX_TRACKED = 100

    /**
     * Whether a scan result for [key] at [now] goes to the core, recording it
     * if so. A result with no link and no service data never goes: the core
     * could not attribute it, and the call would only take its locks.
     */
    fun admit(
        lastFedAt: MutableMap<String, Long>,
        key: String,
        now: Long,
        linked: Boolean,
        hasServiceData: Boolean,
    ): Boolean {
        if (!linked && !hasServiceData) return false
        val last = lastFedAt[key]
        if (last != null && now - last < INTERVAL_MS) return false
        // ponytail: cleared rather than aged out; costs one early reading.
        if (lastFedAt.size >= MAX_TRACKED) lastFedAt.clear()
        lastFedAt[key] = now
        return true
    }

    /**
     * The scan response. With [nodeHash] (the app opted in and the identity
     * is open) it is the hash as service data under [serviceUuid] instead of
     * the bare UUID: 2 + 16 + 8 = 26 of the 31 bytes, where both would need
     * 44. The UUID still travels, inside the service data, the main packet
     * still lists it, and both scanners' gates accept service data under it.
     * Service data under our own 128-bit UUID needs no SIG assignment, unlike
     * a 16-bit UUID or a company id. It names this device to a scanner that
     * hears it, which is why it is opt-in (R29). Without it, the UUID list
     * makes Android devices more reliably visible to iOS, which queries scan
     * responses actively.
     */
    fun scanResponse(serviceUuid: UUID, nodeHash: ByteArray?): AdvertiseData {
        val builder = AdvertiseData.Builder().setIncludeDeviceName(false)
        return if (nodeHash != null) {
            builder.addServiceData(ParcelUuid(serviceUuid), nodeHash).build()
        } else {
            builder.addServiceUuid(ParcelUuid(serviceUuid)).build()
        }
    }
}
