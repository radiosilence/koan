package cc.blit.koan.ui.nowplaying

import cc.blit.koan.ffi.DspInfo
import cc.blit.koan.ffi.StreamFormat
import java.util.Locale

/**
 * "FLAC 24/96", "MP3 44.1 kHz", "FLAC 16/44.1 → 48 · EQ": what the output is
 * being fed, as the Apple apps' format badge says it.
 *
 * DSP says so first. Otherwise, where the platform reports a mixer between
 * koan and the device (`outputMixed`), the badge says what it did: "→ 48"
 * when it resampled, "· mixed" when it kept the rate but still converted and
 * set the volume. Unknown says nothing beyond a rate the device is known to
 * run at.
 */
fun quality(f: StreamFormat): String {
    val parts = mutableListOf(f.codec.uppercase())
    parts += f.bitDepth?.let { "$it/${rate(f.sampleRate)}" } ?: "${rate(f.sampleRate)} kHz"
    if (f.channels.toInt() != 2) parts += "${f.channels}ch"
    val resampled = f.outputSampleRate?.takeIf { it != f.sampleRate }
    resampled?.let { parts += "→ ${rate(it)}" }
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

/** 44100 → "44.1", 96000 → "96". */
private fun rate(hz: UInt): String {
    val khz = hz.toDouble() / 1000
    return String.format(Locale.ROOT, if (khz == Math.rint(khz)) "%.0f" else "%.1f", khz)
}
