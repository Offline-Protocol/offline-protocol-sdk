package com.offlineprotocol.ble

/**
 * Whether the two-minute forced BLE refresh restarts the advertisement.
 *
 * Restarting starts a new advertising set, and the stack gives every set a
 * new random address. A peer linked to us then sees a new device every two
 * minutes, dials it, and waits out its connect timeout, because the Link Layer
 * does not complete a second connection between two devices that already have
 * one (ADR 0030). So the refresh leaves the advertisement alone while a peer
 * is linked and identified.
 *
 * Two exceptions keep that from stranding the phone undiscoverable:
 * - An advertisement the advertiser reports as not running is always
 *   restarted. Skipping the restart while linked used to leave a stopped
 *   advertisement stopped for as long as any link stayed up, so a third
 *   device could not find this phone.
 * - Only identified links hold it. An inbound central that has not proved who
 *   it is, or the server's echo of a dial still in flight, may be closed in a
 *   moment, and must not keep an advertisement from being refreshed.
 *
 * What it cannot see: an advertisement the stack stopped without telling the
 * advertiser. While linked that one is recovered by the next membership
 * change, disconnect or peer loss, each of which refreshes it (ADR 0030 records
 * the trade).
 */
internal object AdvertisingRefreshPolicy {
    fun restartsAdvertising(identifiedLinks: Int, isAdvertising: Boolean): Boolean =
        identifiedLinks == 0 || !isAdvertising
}
