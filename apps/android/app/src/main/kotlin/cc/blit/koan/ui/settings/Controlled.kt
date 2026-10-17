package cc.blit.koan.ui.settings

import cc.blit.koan.engine.Devices
import cc.blit.koan.ffi.DeviceReach

/** How the device being controlled can be reached. */
sealed interface Reach {
    data object Connected : Reach

    /** Being woken: by the network, a push, or a notification to tap there. */
    data class Waking(val stage: String) : Reach

    /** Asleep, and a command wakes it. */
    data object Asleep : Reach

    /** Not heard from for a moment, and wakeable: the connection coming back. */
    data object Reconnecting : Reach

    /** Not reachable: commands fail until it is back. */
    data class OutOfReach(val why: String?) : Reach
}

/** Another device this phone shows and commands, and how it can be reached. */
data class Controlled(val name: String, val reach: Reach) {
    /** What the mini player says in place of the artist: where the music plays. */
    val headline
        get() = when (reach) {
            Reach.Connected -> "Playing on $name"
            is Reach.Waking -> "Waking $name…"
            Reach.Asleep -> "Playing on $name, asleep"
            Reach.Reconnecting -> "Reconnecting to $name…"
            is Reach.OutOfReach -> "$name is out of reach"
        }
}

/**
 * The device being controlled, if one is: `null` while this phone plays. How
 * it can be reached is the engine's reading of its connections.
 */
fun controlled(devices: Devices): Controlled? {
    val target = devices.target ?: return null
    val d = devices.devices.firstOrNull { it.id == target }
    val reach = when (val r = devices.targetReach) {
        DeviceReach.Connected -> Reach.Connected
        DeviceReach.Reconnecting -> Reach.Reconnecting
        DeviceReach.Waking -> Reach.Waking(d?.waking ?: "network")
        DeviceReach.Asleep -> Reach.Asleep
        is DeviceReach.OutOfReach -> Reach.OutOfReach(r.why)
        null -> Reach.OutOfReach(null)
    }
    return Controlled(d?.name ?: "Another device", reach)
}
