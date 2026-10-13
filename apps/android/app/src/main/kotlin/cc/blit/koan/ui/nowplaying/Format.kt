package cc.blit.koan.ui.nowplaying

import cc.blit.koan.ffi.DspInfo
import cc.blit.koan.ffi.StreamFormat
import java.util.Locale

/**
 * "FLAC 24/96", "MP3 44.1 kHz", "FLAC 16/44.1 → 48 · EQ": what the output is
 * being fed, as the Apple apps' format badge says it. The arrow appears only
 * when the device runs at another rate, so something resampled to reach it.
 */
fun quality(f: StreamFormat): String {
    val parts = mutableListOf(f.codec.uppercase())
    parts += f.bitDepth?.let { "$it/${rate(f.sampleRate)}" } ?: "${rate(f.sampleRate)} kHz"
    if (f.channels.toInt() != 2) parts += "${f.channels}ch"
    f.outputSampleRate?.takeIf { it != f.sampleRate }?.let { parts += "→ ${rate(it)}" }
    f.dsp?.let { parts += "· ${dspLabel(it)}" }
    return parts.joinToString(" ")
}

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
