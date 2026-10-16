package cc.blit.koan.playback

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build
import androidx.annotation.RequiresApi
import androidx.core.content.edit
import cc.blit.koan.ffi.AndroidOutputHost
import cc.blit.koan.ffi.MixerEncoding
import cc.blit.koan.ffi.MixerFormat

/**
 * Bit-perfect output to a USB DAC, from Android 14: the engine asks what the
 * DAC offers before each stream and this sets Android's preferred mixer
 * attributes for media, so the mixer hands koan's samples to the DAC
 * unchanged. Everything else goes through the mixer as before.
 *
 * While held, Android silences other apps' sound on the DAC, which is what
 * bit-perfect means there. The choice of format is the engine's
 * (`koan_core::audio::bit_perfect`); this only answers for the Java APIs.
 *
 * Called on the engine's player thread, where an exception would end
 * playback: each answer falls back to the mixer when Android throws, as it
 * may while a USB device is coming or going.
 */
class BitPerfectHost(context: Context) : AndroidOutputHost {
    private val context = context.applicationContext
    private val audio = context.getSystemService(AudioManager::class.java)

    /** What the last `offers` listed, by the format the engine names back. */
    private var offered = emptyMap<MixerFormat, Pair<AudioDeviceInfo, Any>>()
    private var held: AudioDeviceInfo? = null

    override fun mixerRate(): UInt = runCatching {
        audio.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)?.toUIntOrNull()
    }.getOrNull() ?: 48_000u

    override fun offers(): List<MixerFormat> {
        offered = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE && isEnabled(context)) {
            runCatching { bitPerfectFormats() }.getOrDefault(emptyMap())
        } else {
            emptyMap()
        }
        return offered.keys.toList()
    }

    override fun hold(format: MixerFormat): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return false
        val (device, attributes) = offered[format] ?: return false
        // Another DAC's preference would otherwise outlive the switch.
        if (held != null && held?.id != device.id) release()
        val ok = runCatching {
            audio.setPreferredMixerAttributes(MEDIA, device, attributes as AudioMixerAttributes)
        }.getOrDefault(false)
        if (ok) held = device
        return ok
    }

    override fun release() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
        // The device may already be gone, which is often why this is called.
        held?.let { runCatching { audio.clearPreferredMixerAttributes(MEDIA, it) } }
        held = null
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun bitPerfectFormats(): Map<MixerFormat, Pair<AudioDeviceInfo, Any>> =
        audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
            .filter { it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET }
            .flatMap { device ->
                audio.getSupportedMixerAttributes(device)
                    .filter { it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT }
                    .mapNotNull { attributes ->
                        val format = attributes.format
                        val encoding = encoding(format.encoding) ?: return@mapNotNull null
                        MixerFormat(
                            deviceId = device.id,
                            sampleRate = format.sampleRate.toUInt(),
                            channels = format.channelCount.toUShort(),
                            encoding = encoding,
                        ) to (device to attributes)
                    }
            }
            .toMap()

    companion object {
        private val MEDIA = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
        private const val PREFERENCES = "playback"
        private const val KEY = "bit_perfect_usb"

        /** Whether to take a USB DAC bit-perfect when it offers to. On unless turned off. */
        fun isEnabled(context: Context) =
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).getBoolean(KEY, true)

        /** Takes effect at the next stream; `PlayerModel.restartOutput` applies it now. */
        fun setEnabled(context: Context, on: Boolean) =
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE).edit { putBoolean(KEY, on) }

        /** The engine's encodings among Android's; the others cannot be bit-perfect from koan. */
        fun encoding(android: Int) = when (android) {
            AudioFormat.ENCODING_PCM_16BIT -> MixerEncoding.PCM16
            AudioFormat.ENCODING_PCM_24BIT_PACKED -> MixerEncoding.PCM24_PACKED
            AudioFormat.ENCODING_PCM_32BIT -> MixerEncoding.PCM32
            AudioFormat.ENCODING_PCM_FLOAT -> MixerEncoding.FLOAT
            else -> null
        }
    }
}
