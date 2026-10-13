package cc.blit.koan.service

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper

/**
 * Follows where the system sends music, and says so when it changes: a DAC
 * attached, headphones out, Bluetooth in. The service forwards each
 * [OutputRoute] to `setAudioRouteAndroid`, which names the route for the
 * engine's profiles and is the cue to reopen the output on it.
 *
 * The engine's own AAudio error callback reopens the stream when the system
 * disconnects it; this is the half that tells the engine *what* it is now
 * playing to. Main thread: the framework's callback is delivered there.
 */
class RouteWatcher(
    private val context: Context,
    private val onRoute: (OutputRoute, AudioDeviceInfo?) -> Unit,
) {
    private val audioManager = context.getSystemService(AudioManager::class.java)
    private var last: OutputRoute? = null

    private val callback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() {
        audioManager.registerAudioDeviceCallback(callback, Handler(Looper.getMainLooper()))
        refresh()
    }

    fun stop() {
        audioManager.unregisterAudioDeviceCallback(callback)
    }

    /** The current route, read now. */
    fun current(): OutputRoute? = last

    private fun refresh() {
        val devices = candidates()
        val info = chosen(devices)
        val route = OutputRoutes.route(info?.let(::describe), mixerRate())
        if (route == last) return
        last = route
        onRoute(route, info)
    }

    private fun candidates(): List<AudioDeviceInfo> =
        audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).filter { it.isSink }

    private fun chosen(devices: List<AudioDeviceInfo>): AudioDeviceInfo? {
        // From API 33 the system says which device music would use; before
        // that (or if it says nothing) the person's likely intent decides.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val attributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build()
            audioManager.getAudioDevicesForAttributes(attributes).firstOrNull()?.let { return it }
        }
        val best = OutputRoutes.choose(devices.map(::describe)) ?: return null
        return devices.firstOrNull { it.id == best.id }
    }

    private fun describe(info: AudioDeviceInfo) = OutputDevice(info.id, label(info), kind(info.type))

    private fun label(info: AudioDeviceInfo): String =
        info.productName?.toString()?.takeIf { it.isNotBlank() } ?: kind(info.type).name

    private fun mixerRate(): Int =
        audioManager.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)?.toIntOrNull() ?: 0

    companion object {
        fun kind(type: Int): RouteKind = when (type) {
            AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_USB_ACCESSORY -> RouteKind.Usb
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_LINE_ANALOG, AudioDeviceInfo.TYPE_LINE_DIGITAL -> RouteKind.Wired
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLE_HEADSET,
            AudioDeviceInfo.TYPE_BLE_SPEAKER, AudioDeviceInfo.TYPE_BLE_BROADCAST,
            AudioDeviceInfo.TYPE_HEARING_AID -> RouteKind.Bluetooth
            AudioDeviceInfo.TYPE_HDMI, AudioDeviceInfo.TYPE_HDMI_ARC,
            AudioDeviceInfo.TYPE_HDMI_EARC -> RouteKind.Hdmi
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER_SAFE -> RouteKind.Speaker
            else -> RouteKind.Other
        }
    }
}
