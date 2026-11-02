package cc.blit.koan.net

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.PowerManager
import androidx.core.content.ContextCompat
import cc.blit.koan.engine.EngineMirror

/**
 * Tells the engine when the device's default network changes: Wi-Fi to mobile
 * data and back, or none at all. Connections to the server made on the last
 * network are no use on the next, so the engine makes them afresh and tries a
 * server it had given up on at once rather than when its backoff says.
 */
class NetworkWatch(context: Context, private val mirror: EngineMirror) {
    private val lock = Any()
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    /** The network the app started on: answered again at registration, which is no change. */
    private var current: Network? = connectivity.activeNetwork
    private val power = context.getSystemService(PowerManager::class.java)
    @Volatile private var metered = connectivity.isActiveNetworkMetered
    private var told: Boolean? = null

    init {
        connectivity.registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) = moved(network)
                override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                    metered = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
                    tellConstrained()
                }
                override fun onLost(network: Network) {
                    synchronized(lock) { if (current != network) return }
                    moved(null)
                }
            },
        )
        ContextCompat.registerReceiver(
            context,
            object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) = tellConstrained()
            },
            IntentFilter(PowerManager.ACTION_POWER_SAVE_MODE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        tellConstrained()
    }

    /**
     * Background work nobody asked for, such as fetching covers ahead, waits
     * on a metered network or while the device saves power.
     */
    private fun tellConstrained() {
        val constrained = metered || power.isPowerSaveMode
        val changed = synchronized(lock) {
            (told != constrained).also { told = constrained }
        }
        if (changed) mirror.launch { setNetworkConstrained(constrained) }
    }

    private fun moved(network: Network?) {
        val changed = synchronized(lock) {
            val changed = current != network
            current = network
            changed
        }
        if (changed) mirror.launch { networkChanged() }
    }
}
