package cc.blit.koan.ui.nowplaying

import cc.blit.koan.ffi.DspInfo
import cc.blit.koan.ffi.StreamFormat
import java.util.Locale

/**
 * "FLAC 24/96", "MP3 320 kbps", "FLAC 16/44.1 → 48 · EQ": what the output is
 * being fed, as the Apple apps' format badge says it. A lossy file is named by
 * its bitrate where the decoder knows it, since the encoder setting says more
 * about it than the sample rate does.
 *
 * DSP says so first. Otherwise, where the platform reports a mixer between
 * koan and the device (`outputMixed`), the badge says what it did: "→ 48"
 * when it resampled, "· mixed" when it kept the rate but still converted and
 * set the volume. Unknown says nothing beyond a rate the device is known to
 * run at.
 */
fun quality(f: StreamFormat): String {
    val parts = mutableListOf(f.codec.uppercase())
    val kbps = f.bitrateKbps?.takeIf { f.codec.uppercase() in lossy }
    parts += f.bitDepth?.let { "$it/${rate(f.sampleRate)}" } ?: kbps?.let { "$it kbps" } ?: "${rate(f.sampleRate)} kHz"
    if (f.channels.toInt() != 2) parts += "${f.channels}ch"
    val resampled = f.outputSampleRate?.takeIf { it != f.sampleRate }
    resampled?.let { parts += if (kbps == null) "→ ${rate(it)}" else "→ ${rate(it)} kHz" }
    val dsp = f.dsp
    when {
        dsp != null -> parts += "· ${dspLabel(dsp)}"
        f.outputMixed == true && resampled == null -> parts += "· mixed"
    }
    return parts.joinToString(" ")
}

/** The samples reach the device as they are: held bit-perfect, untouched by DSP, at the source's rate. */
fun bitPerfect(f: StreamFormat) = f.outputMixed == false && f.dsp == null && f.outputSampleRate == f.sampleRate

private fun dspLabel(d: DspInfo) = when {
    d.eq && d.convolutionRate != null -> "EQ + FIR"
    d.convolutionRate != null -> "FIR"
    else -> "EQ"
}

/** Codecs whose encoder setting, not the rate, says what the file is. */
private val lossy = setOf("MP3", "AAC", "VORBIS", "OPUS", "OGG", "WMA", "MPC", "MP2")

/** 44100 → "44.1", 96000 → "96". */
private fun rate(hz: UInt): String {
    val khz = hz.toDouble() / 1000
    return String.format(Locale.ROOT, if (khz == Math.rint(khz)) "%.0f" else "%.1f", khz)
}
