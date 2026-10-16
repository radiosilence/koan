package cc.blit.koan.service

import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build

/**
 * Puts a USB DAC's mixer in bit-perfect mode (`MIXER_BEHAVIOR_BIT_PERFECT`,
 * Android 14 / API 34 and later) at the source's sample rate, so the system
 * mixer passes the engine's samples through untouched instead of resampling
 * to 48 kHz. Everywhere else it does nothing: internal speakers, wired
 * headphones and Bluetooth are always resampled.
 *
 * The order matters, and is the service's to keep: the mixer attributes are
 * set *before* the engine opens its stream at the source rate. A source rate
 * the engine is already playing at is not known here until the mirror
 * reports the format, so a change of rate sets the attributes and then
 * re-announces the route, which makes the engine reopen on them.
 *
 * Support varies by manufacturer: a device may list no bit-perfect mode, or
 * refuse to set one. Both read as "not bit-perfect" and the badge says so.
 */
class UsbBitPerfect(private val audioManager: AudioManager) {
    private val attributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_MEDIA)
        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
        .build()

    private var device: AudioDeviceInfo? = null

    /** The rate the mixer was last put in bit-perfect mode at, or null if it is not. */
    var activeRate: Int? = null
        private set

    val supported: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE

    /**
     * Ask for bit-perfect mixing on `route`'s device at `sourceRate`. Returns
     * true if the mixer's mode changed, so the engine should reopen.
     */
    fun prepare(route: OutputRoute, info: AudioDeviceInfo?, sourceRate: Int): Boolean {
        if (!supported || info == null || route.kind != RouteKind.Usb || sourceRate <= 0) {
            return release()
        }
        if (info.id == device?.id && activeRate == sourceRate) return false
        val options = audioManager.getSupportedMixerAttributes(info)
        val modes = options.map { it.toMode() }
        val picked = BitPerfect.pickMixerMode(modes, sourceRate) ?: return release()
        val chosen = options[modes.indexOf(picked)]
        val set = audioManager.setPreferredMixerAttributes(attributes, info, chosen)
        if (!set) return release()
        device = info
        activeRate = sourceRate
        return true
    }

    /** Hand the mixer back to the system. True if it was in our mode. */
    fun release(): Boolean {
        val held = device ?: return false
        if (supported) audioManager.clearPreferredMixerAttributes(attributes, held)
        device = null
        activeRate = null
        return true
    }

    private fun AudioMixerAttributes.toMode() = MixerMode(
        sampleRate = format.sampleRate,
        bitPerfect = mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT,
        encodingRank = when (format.encoding) {
            AudioFormat.ENCODING_PCM_FLOAT -> 5
            AudioFormat.ENCODING_PCM_32BIT -> 4
            AudioFormat.ENCODING_PCM_24BIT_PACKED -> 3
            AudioFormat.ENCODING_PCM_16BIT -> 2
            else -> 1
        },
    )
}
