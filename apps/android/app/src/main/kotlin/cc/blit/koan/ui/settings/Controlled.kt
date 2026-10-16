package cc.blit.koan.ui.settings

import cc.blit.koan.engine.Devices

/** How the device being controlled can be reached. */
sealed interface Reach {
    data object Connected : Reach

    /** Being woken: by the network, a push, or a notification to tap there. */
    data class Waking(val stage: String) : Reach

    /** Asleep, and a command wakes it. */
    data object Asleep : Reach

    /** Not reachable: commands go nowhere until it is back, or this phone plays. */
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
            is Reach.OutOfReach -> "$name is out of reach"
        }
}

/**
 * The device being controlled, if one is: `null` while this phone plays. A
 * target no longer in the list is out of reach, by its id since nothing
 * else of it is known.
 */
fun controlled(devices: Devices): Controlled? {
    val target = devices.target ?: return null
    val d = devices.devices.firstOrNull { it.id == target } ?: return Controlled("Another device", Reach.OutOfReach(null))
    val reach = when {
        d.problem != null -> Reach.OutOfReach(d.problem)
        d.waking != null -> Reach.Waking(d.waking!!)
        d.awake -> Reach.Connected
        // A phone asleep that a push wakes; anything else not awake, a
        // departed Mac among them, is out of reach.
        d.wakeable -> Reach.Asleep
        else -> Reach.OutOfReach(d.wakeFailed)
    }
    return Controlled(d.name, reach)
}
