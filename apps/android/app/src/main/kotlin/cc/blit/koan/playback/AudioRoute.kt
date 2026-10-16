package cc.blit.koan.playback

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import cc.blit.koan.engine.EngineMirror

/**
 * Where Android sends media, named as a person would, told to the engine on
 * every change: the phone's one output in Output and in the profiles, as
 * the route is on iOS.
 */
class AudioRoute(context: Context, private val mirror: EngineMirror) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private var told: String? = null

    init {
        audio.registerAudioDeviceCallback(
            object : AudioDeviceCallback() {
                override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = tell()
                override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = tell()
            },
            Handler(Looper.getMainLooper()),
        )
    }

    private fun tell() {
        val name = name(current() ?: return)
        if (name == told) return
        told = name
        mirror.launch { setAudioRoute(name) }
    }

    /** The device media plays through: Android says on 13 and later; before, the likeliest of those attached. */
    private fun current(): AudioDeviceInfo? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
            audio.getAudioDevicesForAttributes(media).firstOrNull()?.let { return it }
        }
        val outputs = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
        return PRIORITY.firstNotNullOfOrNull { type -> outputs.firstOrNull { it.type == type } }
    }

    private companion object {
        /** What Android routes media to first, when several are attached. */
        val PRIORITY = listOf(
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_USB_DEVICE,
            AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
            AudioDeviceInfo.TYPE_BLE_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER,
        )

        fun name(d: AudioDeviceInfo): String = when (d.type) {
            AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> "Phone speaker"
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> "Headphones"
            // A Bluetooth or USB device's own product name; Android gives the
            // phone's model for a device that has none.
            else -> d.productName?.toString()?.takeIf { it.isNotBlank() && it != Build.MODEL } ?: when (d.type) {
                AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLE_HEADSET -> "Bluetooth"
                AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> "USB audio"
                else -> "Phone speaker"
            }
        }
    }
}
