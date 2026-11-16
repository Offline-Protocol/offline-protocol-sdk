package com.offlineprotocol

import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.net.Inet4Address
import java.net.NetworkInterface

/**
 * SPIKE (BLE-to-Wi-Fi fast link, phase 1). Throwaway: not a design.
 *
 * Driven from adb so the spike needs no UDL or JS surface:
 *
 *     adb shell am broadcast -a com.offlineprotocol.SPIKE_FAST_LINK \
 *         --es op host --es ssid DIRECT-sp-fastlink --es pass <hex> [--ei band 2|5]
 *     ... --es op lohs     (LocalOnlyHotspot; the system picks the credentials)
 *     ... --es op info     (group, Wi-Fi and interface state)
 *     ... --es op stop     (remove the group, close the hotspot)
 *     ... --es op nolan    (stop the LAN carrier: no shared network)
 *
 * Everything it learns goes to logcat under [TAG]. The peer stream itself is
 * the manager's: its listener binds every interface on the group port, so an
 * iPhone that joins the group or the hotspot dials it like any other.
 */
internal class FastLinkSpike(
    private val context: Context,
    /** Stops the LAN carrier, so a pair on one Wi-Fi network meets only over BLE. */
    private val stopLan: () -> Unit = {},
) {
    companion object {
        const val TAG = "OPSPIKE"
        const val ACTION = "com.offlineprotocol.SPIKE_FAST_LINK"
    }

    private val handler = Handler(Looper.getMainLooper())
    private val p2p = context.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
    private val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
    private var channel: WifiP2pManager.Channel? = null
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null
    private var t0 = 0L

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(c: Context, intent: Intent) {
            val op = intent.getStringExtra("op") ?: "info"
            log("op=$op")
            when (op) {
                "host" -> host(
                    intent.getStringExtra("ssid") ?: "DIRECT-sp-fastlink",
                    intent.getStringExtra("pass") ?: "",
                    intent.getIntExtra("band", 0),
                )
                "lohs" -> lohs()
                "stop" -> stopAll()
                "nolan" -> stopLan()
                else -> info()
            }
        }
    }

    fun register() {
        val filter = IntentFilter(ACTION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED)
        } else {
            context.registerReceiver(receiver, filter)
        }
        log("registered")
    }

    fun unregister() {
        try { context.unregisterReceiver(receiver) } catch (_: Exception) {}
        stopAll()
        channel?.close()
        channel = null
    }

    private fun log(message: String) {
        val since = if (t0 == 0L) 0 else SystemClock.elapsedRealtime() - t0
        Log.i(TAG, "wall=${System.currentTimeMillis()} +${since}ms $message")
    }

    private fun ch(): WifiP2pManager.Channel? {
        if (channel == null) channel = p2p?.initialize(context, Looper.getMainLooper(), null)
        return channel
    }

    @SuppressLint("MissingPermission")
    private fun host(ssid: String, pass: String, band: Int) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) { log("host needs API 29"); return }
        val p2p = p2p ?: run { log("no p2p"); return }
        val ch = ch() ?: run { log("no channel"); return }
        t0 = SystemClock.elapsedRealtime()
        val builder = WifiP2pConfig.Builder()
            .setNetworkName(ssid)
            .setPassphrase(pass)
            .enablePersistentMode(false)
        when (band) {
            2 -> builder.setGroupOperatingBand(WifiP2pConfig.GROUP_OWNER_BAND_2GHZ)
            5 -> builder.setGroupOperatingBand(WifiP2pConfig.GROUP_OWNER_BAND_5GHZ)
        }
        val config = builder.build()
        // Whatever group exists goes first: createGroup fails BUSY beside one.
        p2p.removeGroup(ch, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = create()
            override fun onFailure(reason: Int) = create()
            fun create() {
                log("createGroup ssid=$ssid band=$band")
                p2p.createGroup(ch, config, object : WifiP2pManager.ActionListener {
                    override fun onSuccess() {
                        log("createGroup ok")
                        handler.postDelayed({ info() }, 1500)
                        handler.postDelayed({ info() }, 5000)
                    }
                    override fun onFailure(reason: Int) = log("createGroup failed reason=$reason")
                })
            }
        })
    }

    @SuppressLint("MissingPermission")
    private fun lohs() {
        val wifi = wifi ?: run { log("no wifi"); return }
        t0 = SystemClock.elapsedRealtime()
        try {
            wifi.startLocalOnlyHotspot(object : WifiManager.LocalOnlyHotspotCallback() {
                override fun onStarted(r: WifiManager.LocalOnlyHotspotReservation) {
                    reservation = r
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        val c = r.softApConfiguration
                        log("lohs started ssid=${c.ssid} pass=${c.passphrase} security=${c.securityType}")
                    } else {
                        @Suppress("DEPRECATION")
                        val c = r.wifiConfiguration
                        log("lohs started ssid=${c?.SSID} pass=${c?.preSharedKey}")
                    }
                    handler.postDelayed({ info() }, 1500)
                }
                override fun onStopped() = log("lohs stopped")
                override fun onFailed(reason: Int) = log("lohs failed reason=$reason")
            }, handler)
        } catch (e: Exception) {
            log("lohs threw ${e.javaClass.simpleName}: ${e.message}")
        }
    }

    @SuppressLint("MissingPermission")
    private fun stopAll() {
        reservation?.close()
        reservation = null
        val ch = channel ?: return
        p2p?.removeGroup(ch, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = log("group removed")
            override fun onFailure(reason: Int) = log("remove group failed reason=$reason")
        })
    }

    @SuppressLint("MissingPermission")
    private fun info() {
        val ch = ch()
        if (ch != null) {
            p2p?.requestGroupInfo(ch) { g ->
                if (g == null) {
                    log("group none")
                } else {
                    val freq = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) g.frequency else -1
                    log("group name=${g.networkName} pass=${g.passphrase} owner=${g.isGroupOwner} " +
                        "iface=${g.`interface`} freq=$freq clients=${g.clientList.size}")
                }
            }
        }
        @Suppress("DEPRECATION")
        val ci = wifi?.connectionInfo
        @Suppress("DEPRECATION")
        log("wifi enabled=${wifi?.isWifiEnabled} sta ssid=${ci?.ssid} freq=${ci?.frequency} rssi=${ci?.rssi}")
        val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
        log("default network=${cm?.activeNetwork}")
        try {
            for (nif in NetworkInterface.getNetworkInterfaces()) {
                if (!nif.isUp) continue
                val v4 = nif.inetAddresses.toList().filterIsInstance<Inet4Address>()
                if (v4.isNotEmpty()) log("iface ${nif.name} ${v4.joinToString { it.hostAddress ?: "?" }}")
            }
        } catch (e: Exception) {
            log("interfaces: ${e.message}")
        }
    }
}
