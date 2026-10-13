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
 * `MulticastLock`, which costs battery while held. Answers to a renderer
 * search are unicast and need no lock; the engine holds it only for a few
 * seconds after each search, to hear renderers announcing themselves. And a
 * phone with mobile data may route a search out of the wrong interface, so the
 * engine sends from, and listens on, the Wi-Fi address this follows.
 */
class WifiMulticast(context: Context) : MulticastHost {
    private val lock = context.getSystemService(WifiManager::class.java)
        .createMulticastLock("koan-ssdp")
        .apply { setReferenceCounted(false) }

    /** Each Wi-Fi network's IPv4 address, internet or not: a LAN is enough. */
    private val addresses = java.util.concurrent.ConcurrentHashMap<Network, String>()

    init {
        val connectivity = context.getSystemService(ConnectivityManager::class.java)
        val wifi = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        connectivity.registerNetworkCallback(
            wifi,
            object : ConnectivityManager.NetworkCallback() {
                override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
                    val address = properties.linkAddresses
                        .map { it.address }
                        .firstOrNull { it is Inet4Address }
                        ?.hostAddress
                    if (address == null) addresses.remove(network) else addresses[network] = address
                }

                override fun onLost(network: Network) {
                    addresses.remove(network)
                }
            },
        )
    }

    override fun wifiAddress(): String? = addresses.values.firstOrNull()

    override fun holdMulticast(on: Boolean) {
        runCatching { if (on) lock.acquire() else if (lock.isHeld) lock.release() }
    }
}
