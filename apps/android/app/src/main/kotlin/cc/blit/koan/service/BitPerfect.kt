package cc.blit.koan.service

/** What the route was granted, from `KoanEngine.audioOutputGrant()`, without the generated type. */
data class Grant(val exclusive: Boolean, val streamRate: Int, val hardwareRate: Int, val deviceId: Int)

/** Whether the current route is bit-perfect, and if not, why not. The badge says this. */
sealed interface BitPerfectStatus {
    /** The DAC's own stream, at the source's rate. */
    data object Exclusive : BitPerfectStatus

    /** Through the system mixer in bit-perfect mode (API 34+, USB), at the source's rate. */
    data object MixerBitPerfect : BitPerfectStatus

    data class No(val reason: String) : BitPerfectStatus
}

/** One thing a USB output's mixer can be set to (`AudioMixerAttributes`, without the framework type). */
data class MixerMode(val sampleRate: Int, val bitPerfect: Boolean, val encodingRank: Int)

object BitPerfect {
    /**
     * Bit-perfect only if the route is a USB DAC, the hardware runs at the
     * source's rate, and either the stream is the device's own or the mixer
     * was put in bit-perfect mode for it. Judged from what was *granted*
     * (the grant is read back from the stream), never from what was asked for.
     * Internal speakers, wired headphones and Bluetooth always go through the
     * mixer, and are resampled.
     */
    fun status(
        route: OutputRoute?,
        grant: Grant?,
        sourceRate: Int,
        mixerBitPerfectSet: Boolean,
    ): BitPerfectStatus {
        if (route == null || grant == null) return BitPerfectStatus.No("Not playing")
        if (route.kind != RouteKind.Usb) return BitPerfectStatus.No("Only a USB DAC can be bit-perfect")
        if (sourceRate <= 0) return BitPerfectStatus.No("Rate unknown")
        if (grant.streamRate != sourceRate) return BitPerfectStatus.No("Stream is not at the source rate")
        if (grant.hardwareRate != sourceRate) return BitPerfectStatus.No("Resampled to ${grant.hardwareRate} Hz")
        return when {
            grant.exclusive -> BitPerfectStatus.Exclusive
            mixerBitPerfectSet -> BitPerfectStatus.MixerBitPerfect
            else -> BitPerfectStatus.No("Through the system mixer")
        }
    }

    /**
     * Of the modes a USB output supports, the one to ask for so that the mixer
     * runs at `sourceRate` untouched: bit-perfect behaviour, that rate, and the
     * widest sample format (the engine's samples are float32, so the least
     * conversion). Null when the DAC cannot do that rate bit-perfectly.
     */
    fun pickMixerMode(modes: List<MixerMode>, sourceRate: Int): MixerMode? =
        modes.filter { it.bitPerfect && it.sampleRate == sourceRate }.maxByOrNull { it.encodingRank }
}
