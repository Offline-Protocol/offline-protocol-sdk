package com.offlineprotocol

import java.security.MessageDigest

/**
 * Names the application behind one instance of the Offline Protocol GATT
 * service, so a central can tell its own app's instance from another's.
 *
 * Mirrors iOS's BleAppTag.swift. Keep in sync.
 *
 * ## Why a service instance needs naming at all
 *
 * Every app built on this SDK registers the same service UUID, and iOS and
 * Android merge every app's GATT service into one device-wide database. A phone
 * running two SDK apps therefore presents two instances of the one service
 * behind one BLE link, each serving a different identity. Without a name, a
 * central binds to whichever instance it happens to reach first, and on a phone
 * running a chat app and a game that is the chat app half the time.
 *
 * ## Why a hash, and why eight bytes
 *
 * The characteristic is readable by anything in radio range, so it carries a
 * digest rather than the developer's `appId` string. Eight bytes is one ATT
 * read at the minimum MTU with no long-read path. A collision between two apps
 * on one phone costs only the pre-tag behaviour: the central may bind to the
 * other app's instance, whose address the handshake that follows still proves
 * against its key.
 *
 * ## Why it is not signed
 *
 * Any identity key can sign any tag, so a signature would prove only that the
 * instance holds some key, which the IDENTITY characteristic already proves. An
 * app co-installed on the peer's phone that serves someone else's tag is the
 * malicious-application adversary (A5 in the threat model), which the SDK does
 * not defend an application against.
 */
object BleAppTag {

    /** Bytes served in the characteristic. */
    const val SIZE = 8

    /**
     * Domain separator, so the tag is never the prefix of a digest computed for
     * any other purpose over the same `appId`.
     */
    private const val DOMAIN = "offline-protocol-ble-app-tag-v1"

    /** `SHA-256(DOMAIN || 0x00 || appId)[0..8]`. */
    @JvmStatic
    fun compute(appId: String): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
            .digest("$DOMAIN\u0000$appId".toByteArray(Charsets.UTF_8))
        return digest.copyOf(SIZE)
    }
}
