package cc.blit.koan.service

/** The kinds of output the route policy tells apart. */
enum class RouteKind { Usb, Wired, Bluetooth, Hdmi, Speaker, Other }

/** One output device, as far as route choice cares: no `AudioDeviceInfo`, so it can be tested. */
data class OutputDevice(val id: Int, val name: String, val kind: RouteKind)

/** What the engine is told on a route change: `setAudioRouteAndroid(name, id, rate)`. */
data class OutputRoute(val name: String, val deviceId: Int, val mixerRate: Int, val kind: RouteKind)

/**
 * Which of the attached outputs music goes to.
 *
 * Android has the final say, and on API 33+ the service asks it
 * (`getAudioDevicesForAttributes`). This is the answer for older releases and
 * the tie-break: what a person who plugged something in expects. A DAC, then
 * wired headphones, then Bluetooth, then the television, then the speaker.
 */
object OutputRoutes {
    private val preference = listOf(
        RouteKind.Usb, RouteKind.Wired, RouteKind.Bluetooth, RouteKind.Hdmi, RouteKind.Speaker, RouteKind.Other,
    )

    fun choose(devices: List<OutputDevice>): OutputDevice? =
        devices.minWithOrNull(compareBy({ preference.indexOf(it.kind) }, { it.id }))

    /**
     * `mixerRate` is the rate the system mixer runs at for this route (what a
     * shared stream is resampled to), which the engine reports as the
     * hardware rate when it cannot read one back. Zero when the platform does
     * not say.
     */
    fun route(device: OutputDevice?, mixerRate: Int): OutputRoute =
        if (device == null) OutputRoute("Default output", 0, mixerRate, RouteKind.Other)
        else OutputRoute(device.name, device.id, mixerRate, device.kind)
}
