package cc.blit.koan.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.wifi.WifiManager
import cc.blit.koan.ffi.MulticastHost
import java.net.Inet4Address

/**
 * The Wi-Fi network, as UPnP discovery needs it.
 *
 * Android drops multicast addressed to the phone unless an app holds a
 * `MulticastLock`, which costs battery while held: the engine holds it only
 * for the few seconds a renderer search's answers arrive in. And a phone with
 * mobile data may route a search out of the wrong interface, so the engine
 * sends from the Wi-Fi address this follows.
 */
class WifiMulticast(context: Context) : MulticastHost {
    private val lock = context.getSystemService(WifiManager::class.java)
        .createMulticastLock("koan-ssdp")
        .apply { setReferenceCounted(false) }

    @Volatile
    private var address: String? = null

    init {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        val wifi = NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build()
        connectivity.registerNetworkCallback(
            wifi,
            object : ConnectivityManager.NetworkCallback() {
                override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
                    address = properties.linkAddresses
                        .map { it.address }
                        .firstOrNull { it is Inet4Address }
                        ?.hostAddress
                }

                override fun onLost(network: Network) {
                    address = null
                }
            },
        )
    }

    override fun wifiAddress(): String? = address

    override fun holdMulticast(on: Boolean) {
        runCatching { if (on) lock.acquire() else if (lock.isHeld) lock.release() }
    }
}
