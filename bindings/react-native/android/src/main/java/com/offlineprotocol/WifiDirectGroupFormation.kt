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
 * presents the same two, with no prompt on either side. Both are derived here
 * from the application id alone, so every device of an application can join
 * its group without having heard who owns it.
 *
 * The passphrase is not a secret. The application id ships inside every copy
 * of the application, so anyone who knows it computes the passphrase, and can
 * join the group or run one under its name (threat model R22). What the
 * passphrase does is keep honest devices of other applications out of each
 * other's groups. It is not what protects the traffic: every stream still
 * proves its peer with the identity preamble before it carries anything, and
 * every message is end to end encrypted above it.
 *
 * ## Why the name is the application's and not the owner's
 *
 * Discovery is asymmetric on real phones. A device that is a group owner, or
 * is in the middle of joining one, answers no Wi-Fi P2P service discovery
 * query, and two phones in range often hear each other's record minutes
 * apart. An earlier version named the group after its owner's address, so a
 * joiner could not join until it had heard the owner; on two phones that
 * stalled formation for minutes, and once for good. A name every device can
 * derive needs nothing heard from the owner.
 *
 * ## Who creates the group
 *
 * Each device advertises the stream chapter's `_offlineprotocol._tcp` record
 * (`docs/spec/stream-framing.md`, `txtvers=1` and `addr=`) plus [KEY_APP], and
 * a device in no group decides from what it has heard:
 *
 * 1. Nothing heard and no group owner nearby: wait.
 * 2. The lowest address among the peers it has heard: create the group, unless
 *    a group owner is nearby, in which case join first (it may be ours).
 * 3. Otherwise join, and take over creating after [TAKEOVER_FAILED_JOINS]
 *    joins found no group: the lower peer may never have heard this one.
 * 4. Only a group owner nearby, no record heard: join, but only as a probe at
 *    most once a minute ([Local.ownerProbeDue]). A Wi-Fi Direct television is a
 *    group owner too, and a device that joins on every step answers no
 *    discovery query for most of its time: on two phones next to two
 *    televisions, both did that and neither heard the other for minutes.
 *
 * A device in a group does nothing here; the manager dissolves a group it
 * owns that stays empty, and the device then joins before it may create
 * again ([Local.joinFirst]), so two groups that formed at once merge.
 *
 * A record proves nothing about its advertiser (the stream chapter's rule), and
 * none is needed: the worst a forged record does is steer whether this device
 * creates or joins, and the worst a group run by a forger does is carry no
 * traffic, since every stream over it still proves its peer (threat model R22).
 *
 * ## Quiet surroundings cost little
 *
 * A device alone keeps looking, but less often the longer it hears nothing:
 * service discovery backs off from [DISCOVERY_PERIOD_MS] to
 * [DISCOVERY_MAX_PERIOD_MS] ([discoveryPeriodMs]), and a probe toward an owner
 * nobody vouched for backs off from [PROBE_PERIOD_MS] to [PROBE_MAX_PERIOD_MS]
 * ([probePeriodMs]). A Wi-Fi Direct printer or television is an owner whose
 * group a probe never joins; without the backoff a device next to one probed
 * it every minute for as long as the application was open. Both start over
 * when something new appears: a record, a new owner, or a new device. An
 * owner counts as new only after [OWNER_MEMORY_MS] out of the peer list, so
 * a television at the edge of range does not re-arm the fast probe each time
 * it flickers back in ([ownersNewlySeen]).
 *
 * ## An owner that was left is never stayed with
 *
 * Every group of an application has the same name, so a join by name lands on
 * whichever owner of that name the supplicant picks (by signal), and the one
 * in sight after a client leaves a group whose owner proved nothing (its
 * application died; the group can outlive the process) is usually the owner
 * just left: the device rejoined it ten seconds later and was held again.
 *
 * A join cannot be steered away from it. `WifiP2pConfig.Builder.setDeviceAddress`
 * on a join by credentials becomes the supplicant's `go_bssid`, which must
 * equal the owner's BSSID, its P2P *interface* address; the peer list and the
 * group report the *device* address, and no application API exposes an
 * owner's interface address before joining it. A join named that way matches
 * no network and fails, every time.
 *
 * So the owner left is remembered for [DEAD_OWNER_TTL_MS] by device address
 * and handled after the fact: it does not count as an owner nearby (no probe
 * goes toward it), a join that lands in its group leaves at once and counts
 * as a join that found no group ([joinedLeftOwner],
 * [failedJoinsAfterGroupJoined]), so a device that heard a peer still takes
 * over and creates after [TAKEOVER_FAILED_JOINS], and its record heard again
 * forgives it. Which owner a join lands on beside a dead one stays the
 * supplicant's choice: the merge there is bounded by the takeover, not
 * guaranteed. The memory outlasts the 30 to 60 s an owner whose application
 * comes back takes to dissolve its empty group.
 */
internal object WifiDirectGroupFormation {
    const val SERVICE_TYPE = "_offlineprotocol._tcp"
    const val INSTANCE_NAME = "offlineprotocol"

    const val KEY_VERSION = "txtvers"
    const val KEY_ADDRESS = "addr"
    /** A tag of the application id; devices of other applications are ignored. */
    const val KEY_APP = "app"
    /** Present on service instances (the DNS-SD mapping chapter); never a peer. */
    const val KEY_SERVICE_ID = "sid"

    /** How many joins that found no group before a non-lowest device creates. */
    const val TAKEOVER_FAILED_JOINS = 3

    /** Service discovery period while records keep arriving. */
    const val DISCOVERY_PERIOD_MS = 15_000L
    /** Service discovery period after a long run of rounds that heard nothing. */
    const val DISCOVERY_MAX_PERIOD_MS = 60_000L
    /** Probe period toward an owner nobody vouched for, before any probe failed. */
    const val PROBE_PERIOD_MS = 60_000L
    /** Probe period after a long run of probes that found no group. */
    const val PROBE_MAX_PERIOD_MS = 240_000L
    /** How long an owner left for proving nothing is avoided. */
    const val DEAD_OWNER_TTL_MS = 300_000L
    /** How long an owner must be out of the peer list to count as new again. */
    const val OWNER_MEMORY_MS = 300_000L

    /** A peer's record as last seen. */
    data class Advert(
        val device: String,
        val address: String,
        val app: String,
        val seenAtMs: Long,
    )

    /** What this device knows about itself when it decides. */
    data class Local(
        val address: String,
        val app: String,
        val inGroup: Boolean,
        /** Consecutive join attempts that ended without a group. */
        val failedJoins: Int = 0,
        /** Any Wi-Fi Direct group owner in the peer list, of any application. */
        val ownerNearby: Boolean = false,
        /** Whether a probe join toward an owner nobody vouched for is due. */
        val ownerProbeDue: Boolean = true,
        /**
         * Join before creating, whatever else holds. Set after this device
         * dissolved an empty group: the other device may own one under the
         * same name, and creating again at once is how two empty groups
         * formed and dissolved in step on two phones.
         */
        val joinFirst: Boolean = false,
    )

    enum class Action { Wait, Create, Join }

    /**
     * Whether the group this client just joined is the application's and
     * owned by a device it left for proving nothing, whose memory is live.
     */
    fun joinedLeftOwner(applicationGroup: Boolean, owner: String?, avoided: Set<String>): Boolean =
        applicationGroup && owner != null && owner in avoided

    /**
     * The failed-join count after joining a group. A group owned by a device
     * this one left is a join that found no group: it counts toward the
     * takeover rather than starting it over, or every capture by the dead
     * owner zeroes the count and the device never creates.
     */
    fun failedJoinsAfterGroupJoined(previous: Int, leftOwner: Boolean): Int =
        if (leftOwner) previous + 1 else 0

    /**
     * The owners in [owners] not seen within [OWNER_MEMORY_MS] before
     * [nowMs], by [lastSeenMs]. A probe backoff starts over only for these.
     */
    fun ownersNewlySeen(owners: Set<String>, lastSeenMs: Map<String, Long>, nowMs: Long): Set<String> =
        owners.filterTo(HashSet()) { owner ->
            val seen = lastSeenMs[owner]
            seen == null || nowMs - seen >= OWNER_MEMORY_MS
        }

    fun decide(local: Local, adverts: Collection<Advert>): Action {
        if (local.inGroup) return Action.Wait
        val peers = adverts.filter { it.app == local.app && it.address != local.address }
        if (peers.isEmpty()) {
            return if (local.ownerNearby && local.ownerProbeDue) Action.Join else Action.Wait
        }
        val lowest = peers.minOf { it.address }
        return when {
            local.joinFirst -> Action.Join
            local.address < lowest ->
                if (local.ownerNearby && local.failedJoins == 0) Action.Join else Action.Create
            local.failedJoins >= TAKEOVER_FAILED_JOINS -> Action.Create
            else -> Action.Join
        }
    }

    /** The discovery period after [quietRounds] rounds in a row that heard no record. */
    fun discoveryPeriodMs(quietRounds: Int): Long =
        doubled(DISCOVERY_PERIOD_MS, quietRounds, DISCOVERY_MAX_PERIOD_MS)

    /** The probe period after [failedProbes] probes in a row that found no group. */
    fun probePeriodMs(failedProbes: Int): Long =
        doubled(PROBE_PERIOD_MS, failedProbes, PROBE_MAX_PERIOD_MS)

    /**
     * Whether formation is on after an enable. A configuration that does not
     * name `autoAccept` keeps the current setting: the integration guide's
     * `enableTransport('wifiDirect')` after a permission grant passes none, and
     * reading that as "off" turned formation off for the rest of the session.
     */
    fun formGroupsAfterEnable(current: Boolean, configured: Boolean?): Boolean =
        configured ?: current

    private fun doubled(base: Long, times: Int, ceiling: Long): Long {
        var period = base
        repeat(times.coerceAtLeast(0)) {
            if (period >= ceiling) return ceiling
            period *= 2
        }
        return minOf(period, ceiling)
    }

    /** The record this device advertises, in the order the chapter requires. */
    fun txtRecord(address: String, appTag: String): LinkedHashMap<String, String> {
        val txt = LinkedHashMap<String, String>()
        txt[KEY_VERSION] = "1"
        txt[KEY_ADDRESS] = address
        txt[KEY_APP] = appTag
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
        return Advert(device, address, app, nowMs)
    }

    /** Eight hex digits of the application id: enough to keep applications apart. */
    fun appTag(appId: String): String = hex(sha256("offline-protocol/wifi-direct/app|$appId"), 4)

    /**
     * The application's group name. Android requires `DIRECT-` and two
     * characters at the front; the result is 18 bytes, under the 32 a network
     * name may hold.
     */
    fun networkName(appId: String): String =
        "DIRECT-op-" + hex(sha256("offline-protocol/wifi-direct/net|$appId"), 4)

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
