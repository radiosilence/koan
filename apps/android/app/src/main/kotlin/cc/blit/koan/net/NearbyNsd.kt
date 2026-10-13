package cc.blit.koan.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.KoanEngine
import cc.blit.koan.ffi.NearbyHost
import cc.blit.koan.ffi.TxtEntry
import java.net.Inet4Address
import java.util.concurrent.Executors
import kotlinx.coroutines.channels.Channel

/**
 * Finding koan devices on the network, and being found: Android's
 * `NsdManager` doing what Bonjour does on Apple platforms. The engine says
 * when to announce this device and when to look; what is found goes back to
 * it, which dials it as it dials a device Bonjour found.
 *
 * Every call here is an event, the engine's or `NsdManager`'s: nothing polls.
 * Only IPv4 addresses are handed on, as the engine dials `host:port`.
 */
class NearbyNsd(context: Context, private val mirror: EngineMirror) : NearbyHost {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val lock = Any()
    private var registration: NsdManager.RegistrationListener? = null
    private var discovery: NsdManager.DiscoveryListener? = null
    /** Android 14: one callback per service, telling of each change to it. */
    private val watching = mutableMapOf<String, NsdManager.ServiceInfoCallback>()
    /** Before Android 14, one resolve at a time: the rest wait their turn. */
    private val waiting = ArrayDeque<NsdServiceInfo>()
    private var resolving = false
    private val executor = Executors.newSingleThreadExecutor()
    /** What was found and lost, told to the engine in the order it happened. */
    private val events = Channel<KoanEngine.() -> Unit>(Channel.UNLIMITED)

    init {
        mirror.launch { for (event in events) runCatching { event() } }
    }

    override fun advertise(name: String, port: UShort, txt: List<TxtEntry>) {
        withdraw()
        val info = runCatching {
            NsdServiceInfo().apply {
                serviceName = name
                serviceType = SERVICE
                this.port = port.toInt()
                txt.forEach { setAttribute(it.key, it.value) }
            }
        }.getOrElse {
            log("cannot announce this device: $it")
            return
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) =
                log("announced as ${info.serviceName}")
            override fun onRegistrationFailed(info: NsdServiceInfo, error: Int) {
                log("cannot announce this device ($error)")
                refused(error)
            }
            override fun onServiceUnregistered(info: NsdServiceInfo) {}
            override fun onUnregistrationFailed(info: NsdServiceInfo, error: Int) {}
        }
        synchronized(lock) { registration = listener }
        runCatching { nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener) }
            .onFailure { log("cannot announce this device: $it") }
    }

    override fun withdraw() {
        val listener = synchronized(lock) { registration.also { registration = null } } ?: return
        runCatching { nsd.unregisterService(listener) }
    }

    override fun browse() {
        val listener = synchronized(lock) {
            if (discovery != null) return
            object : NsdManager.DiscoveryListener {
                private fun current() = synchronized(lock) { discovery === this }
                override fun onDiscoveryStarted(type: String) = log("browsing")
                override fun onStartDiscoveryFailed(type: String, error: Int) {
                    log("cannot browse ($error)")
                    refused(error)
                    synchronized(lock) { if (discovery === this) discovery = null }
                }
                override fun onDiscoveryStopped(type: String) {}
                override fun onStopDiscoveryFailed(type: String, error: Int) {}
                override fun onServiceFound(info: NsdServiceInfo) {
                    if (current()) resolve(info)
                }
                override fun onServiceLost(info: NsdServiceInfo) {
                    if (current()) lost(info.serviceName)
                }
            }.also { discovery = it }
        }
        runCatching { nsd.discoverServices(SERVICE, NsdManager.PROTOCOL_DNS_SD, listener) }
            .onFailure {
                log("cannot browse: $it")
                synchronized(lock) { if (discovery === listener) discovery = null }
            }
    }

    override fun pause() {
        val (listener, callbacks) = synchronized(lock) {
            val l = discovery
            discovery = null
            waiting.clear()
            val c = watching.values.toList()
            watching.clear()
            l to c
        }
        listener?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            callbacks.forEach { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        }
    }

    private fun resolve(info: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            watch(info)
        } else {
            synchronized(lock) {
                waiting.addLast(info)
                if (resolving) return
                resolving = true
            }
            resolveNext()
        }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watch(info: NsdServiceInfo) {
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(error: Int) =
                log("cannot resolve ${info.serviceName} ($error)")
            override fun onServiceUpdated(updated: NsdServiceInfo) {
                val host = updated.hostAddresses.firstOrNull { it is Inet4Address } ?: return
                found(updated, host.hostAddress ?: return)
            }
            override fun onServiceLost() = lost(info.serviceName, this)
            override fun onServiceInfoCallbackUnregistered() {}
        }
        val previous = synchronized(lock) { watching.put(info.serviceName, callback) }
        previous?.let { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        runCatching { nsd.registerServiceInfoCallback(info, executor, callback) }
            .onFailure { log("cannot resolve ${info.serviceName}: $it") }
    }

    @Suppress("DEPRECATION")
    private fun resolveNext() {
        val info = synchronized(lock) {
            waiting.removeFirstOrNull().also { if (it == null) resolving = false }
        } ?: return
        val listener = object : NsdManager.ResolveListener {
            override fun onResolveFailed(info: NsdServiceInfo, error: Int) {
                log("cannot resolve ${info.serviceName} ($error)")
                resolveNext()
            }
            override fun onServiceResolved(resolved: NsdServiceInfo) {
                try {
                    (resolved.host as? Inet4Address)?.hostAddress?.let { found(resolved, it) }
                } finally {
                    resolveNext()
                }
            }
        }
        runCatching { nsd.resolveService(info, listener) }.onFailure { resolveNext() }
    }

    private fun found(info: NsdServiceInfo, host: String) {
        val txt = info.attributes.mapNotNull { (key, value) ->
            value?.let { TxtEntry(key, it.decodeToString()) }
        }
        val name = info.serviceName
        val port = info.port.toUShort()
        events.trySend { nearbyFound(name, host, port, txt) }
    }

    /** Gone: `callback` is the watch that saw it go, if one did. */
    private fun lost(name: String, callback: NsdManager.ServiceInfoCallback? = null) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val ended = synchronized(lock) {
                if (callback == null) watching.remove(name)
                else callback.takeIf { watching.remove(name, it) }
            }
            ended?.let { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        }
        events.trySend { nearbyLost(name) }
    }

    /** Android 17 keeps an app without the local network permission off it. */
    private fun refused(error: Int) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.CINNAMON_BUN &&
            error == NsdManager.FAILURE_PERMISSION_DENIED
        ) {
            events.trySend { nearbyBlocked(true) }
        }
    }

    private fun log(message: String) {
        Log.i("koan", "nearby (nsd): $message")
    }

    private companion object {
        const val SERVICE = "_koan._tcp"
    }
}
