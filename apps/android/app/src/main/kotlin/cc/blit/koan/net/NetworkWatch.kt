package cc.blit.koan.net

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import cc.blit.koan.engine.EngineMirror

/**
 * Tells the engine when the device's default network changes: Wi-Fi to mobile
 * data and back, or none at all. Connections to the server made on the last
 * network are no use on the next, so the engine makes them afresh and tries a
 * server it had given up on at once rather than when its backoff says.
 */
class NetworkWatch(context: Context, private val mirror: EngineMirror) {
    private val lock = Any()
    private var current: Network? = null
    private var started = false

    init {
        context.getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) = moved(network)
                override fun onLost(network: Network) {
                    synchronized(lock) { if (current != network) return }
                    moved(null)
                }
            },
        )
    }

    /** The first answer is the network the app started on, not a change. */
    private fun moved(network: Network?) {
        val changed = synchronized(lock) {
            val changed = started && current != network
            current = network
            started = true
            changed
        }
        if (changed) mirror.launch { networkChanged() }
    }
}
