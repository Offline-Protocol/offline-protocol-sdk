package com.offlineprotocol

import java.security.MessageDigest

/**
 * How Android devices running the same app put themselves into one Wi-Fi
 * Direct group without anyone opening the system settings. Pure, so
 * [WifiDirectGroupFormationTest] pins it without a radio.
 *
 * ## The group is formed by credentials, never by invitation
 *
 * `WifiP2pManager.connect` with a peer's device address starts a negotiation
 * that shows the other phone a system "Invitation to connect" dialog, which a
 * phone in a pocket never answers. Since Android 10 a group can instead be
 * created with a chosen network name and passphrase, and joined by anyone who
 * presents the same two, with no prompt on either side. Both are derived here:
 * the name from the owner's address, the passphrase from the application id
 * and the name, so a device of another application never computes it and a
 * device of this one never needs it sent.
 *
 * The passphrase keeps other applications' devices out of the group; it is not
 * what protects the traffic. Every stream still proves its peer with the
 * identity preamble before it carries anything, and every message is end to
 * end encrypted above it, so a device that joined the group with a leaked
 * passphrase reaches a listener that refuses it at the first frame.
 *
 * ## Who owns the group
 *
 * Each device advertises a DNS-SD record over Wi-Fi P2P service discovery:
 * the `_offlineprotocol._tcp` record the stream chapter
 * (`docs/spec/stream-framing.md`) fixes, `txtvers=1` and `addr=`, plus
 * [KEY_APP] to keep applications apart and,
 * while it owns a group, [KEY_NETWORK] with the group's name. The rules,
 * applied by every device to the same records, converge on one owner:
 *
 * 1. A device in no group joins the owner with the lowest address it can see.
 * 2. With no owner in sight, the device with the lowest address among the
 *    peers it can see creates a group, and every other device joins the group
 *    that one will create, by the name derived from its address. The joiner
 *    does not wait for the owner's updated record: on two phones a group
 *    owner answered no service discovery query at all (it listens on its
 *    operating channel, not the social ones the queries go out on), so a
 *    record saying "I own DIRECT-op-…" never arrived. A join that runs ahead
 *    of the group fails and is retried on the manager's backoff.
 * 3. An owner with no clients that sees an owner with a lower address leaves
 *    its own group and joins that one, so two groups that formed at once
 *    merge, when the other owner's record is heard at all.
 * 4. A device that is a client, or an owner with clients, does nothing.
 *
 * A record proves nothing about its advertiser (the stream chapter's rule), and
 * none is needed here: the worst a forged record can do is name a group that
 * will not accept this device's passphrase, or keep this device waiting for an
 * owner that never appears, and the manager's retry backoff bounds both.
 */
internal object WifiDirectGroupFormation {
    const val SERVICE_TYPE = "_offlineprotocol._tcp"
    const val INSTANCE_NAME = "offlineprotocol"

    const val KEY_VERSION = "txtvers"
    const val KEY_ADDRESS = "addr"
    /** A tag of the application id; devices of other applications are ignored. */
    const val KEY_APP = "app"
    /** Present only while the advertiser owns a group: that group's network name. */
    const val KEY_NETWORK = "net"
    /** Present on service instances (the DNS-SD mapping chapter); never a peer. */
    const val KEY_SERVICE_ID = "sid"

    /** A peer's record as last seen. [ownerNetwork] is null unless it owns a group. */
    data class Advert(
        val device: String,
        val address: String,
        val app: String,
        val ownerNetwork: String?,
        val seenAtMs: Long,
    )

    /** What this device knows about itself when it decides. */
    data class Local(
        val address: String,
        val app: String,
        val inGroup: Boolean,
        val isOwner: Boolean,
        val clients: Int,
    )

    sealed interface Action {
        /** Nothing to do now. */
        object Wait : Action
        /** Create a group under [networkName] with this device as its owner. */
        object Create : Action
        /** Join [owner]'s group, named [network]. */
        data class Join(val network: String, val owner: Advert) : Action
        /** Leave this device's empty group and join [owner]'s, named [network]. */
        data class Move(val network: String, val owner: Advert) : Action
    }

    fun decide(local: Local, adverts: Collection<Advert>): Action {
        val peers = adverts.filter { it.app == local.app && it.address != local.address }
        val owners = peers.filter { it.ownerNetwork != null }
        if (local.inGroup) {
            if (!local.isOwner || local.clients > 0) return Action.Wait
            val lower = owners.filter { it.address < local.address }.minByOrNull { it.address }
                ?: return Action.Wait
            return Action.Move(lower.ownerNetwork!!, lower)
        }
        owners.minByOrNull { it.address }?.let { return Action.Join(it.ownerNetwork!!, it) }
        val lowest = peers.minByOrNull { it.address } ?: return Action.Wait
        return if (local.address < lowest.address) {
            Action.Create
        } else {
            Action.Join(networkName(lowest.address), lowest)
        }
    }

    /** The record this device advertises, in the order the chapter requires. */
    fun txtRecord(address: String, appTag: String, ownerNetwork: String?): LinkedHashMap<String, String> {
        val txt = LinkedHashMap<String, String>()
        txt[KEY_VERSION] = "1"
        txt[KEY_ADDRESS] = address
        txt[KEY_APP] = appTag
        if (ownerNetwork != null) txt[KEY_NETWORK] = ownerNetwork
        return txt
    }

    /**
     * Reads a peer's record, or null for one that is not a peer record of this
     * protocol: another version, no address, or a service instance.
     */
    fun parse(device: String, txt: Map<String, String>, nowMs: Long): Advert? {
        if (txt[KEY_VERSION] != "1") return null
        if (txt.containsKey(KEY_SERVICE_ID)) return null
        val address = txt[KEY_ADDRESS]?.takeIf { it.isNotEmpty() } ?: return null
        val app = txt[KEY_APP] ?: return null
        val network = txt[KEY_NETWORK]?.takeIf { isValidNetworkName(it) }
        return Advert(device, address, app, network, nowMs)
    }

    /** Eight hex digits of the application id: enough to keep applications apart. */
    fun appTag(appId: String): String = hex(sha256("offline-protocol/wifi-direct/app|$appId"), 4)

    /**
     * The group's network name, from its owner's address. Android requires
     * `DIRECT-` and two characters at the front; the result is 18 bytes, under
     * the 32 a network name may hold.
     */
    fun networkName(ownerAddress: String): String =
        "DIRECT-op-" + hex(sha256("offline-protocol/wifi-direct/net|$ownerAddress"), 4)

    /** The group's passphrase: 32 hex digits, inside WPA2's 8 to 63. */
    fun passphrase(appId: String, network: String): String =
        hex(sha256("offline-protocol/wifi-direct/psk|$appId|$network"), 16)

    fun isValidNetworkName(name: String): Boolean =
        name.length <= 32 && Regex("^DIRECT-[a-zA-Z0-9]{2}.*").matches(name)

    private fun sha256(text: String): ByteArray =
        MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))

    private fun hex(bytes: ByteArray, count: Int): String =
        bytes.take(count).joinToString("") { "%02x".format(it.toInt() and 0xff) }
}
