package cc.blit.koan.ui.queue

import uniffi.koan_ffi.QueueItem
import uniffi.koan_ffi.StreamFormat
import java.util.Locale

/** The words and figures the queue and the player show; the port of the Swift `Format`. */
object QueueFormat {
    private val datedTitle = Regex("""^\d{4}-\d{2}-\d{2}\s*[:\-–]\s*""")

    /**
     * A title without the leading date DJ-mix uploads carry ("2022-03-12:
     * BBC Radio 1 Essential Mix"), whose year is shown beside it anyway.
     * Display only: the tags keep it.
     */
    fun title(raw: String): String {
        val rest = datedTitle.replaceFirst(raw, "")
        return if (rest.isEmpty()) raw else rest
    }

    /** `m:ss`, or `h:mm:ss` once it earns the hour. `--:--` for nothing. */
    fun duration(ms: Long?): String = if (ms == null || ms <= 0) "--:--" else clock(ms)

    fun clock(ms: Long): String {
        val total = (ms / 1000).toInt().coerceAtLeast(0)
        val (h, m, s) = Triple(total / 3600, total % 3600 / 60, total % 60)
        return if (h > 0) String.format(Locale.ROOT, "%d:%02d:%02d", h, m, s) else String.format(Locale.ROOT, "%d:%02d", m, s)
    }

    fun count(n: Long, singular: String, plural: String = singular + "s"): String =
        String.format(Locale.ROOT, "%,d", n) + " " + if (n == 1L) singular else plural

    /** 44100 gives "44.1", 96000 gives "96": a trailing ".0" is noise. */
    fun rate(hz: Long): String {
        val khz = hz / 1000.0
        return if (khz == Math.rint(khz)) String.format(Locale.ROOT, "%.0f", khz) else String.format(Locale.ROOT, "%.1f", khz)
    }

    /**
     * What the source is: "FLAC 24/96" and so on. A rate the device refused
     * is appended ("FLAC 24/96 → 48"): a badge that reads the same whether or
     * not something resampled is the one claim this player cannot afford to
     * get wrong. DSP is named for the same reason.
     */
    fun quality(f: StreamFormat): String {
        val parts = mutableListOf(f.codec.uppercase())
        val depth = f.bitDepth
        parts += if (depth != null) "$depth/${rate(f.sampleRate.toLong())}" else "${rate(f.sampleRate.toLong())} kHz"
        if (f.channels.toInt() != 2) parts += "${f.channels}ch"
        val out = f.outputSampleRate
        if (isResampled(f) && out != null) parts += "→ ${rate(out.toLong())}"
        f.dsp?.let { dsp ->
            parts += "· " + when {
                dsp.eq && dsp.convolutionRate != null -> "EQ + FIR"
                dsp.convolutionRate != null -> "FIR"
                else -> "EQ"
            }
        }
        return parts.joinToString(" ")
    }

    /** Whether anything resampled to reach the device. False while its rate is unknown: silence beats a guess. */
    fun isResampled(f: StreamFormat): Boolean = f.outputSampleRate?.let { it != f.sampleRate } == true

    /** What koan can honestly say about the stream's path. It never claims bit-perfection outright. */
    fun outputExplanation(f: StreamFormat): String {
        val dsp = f.dsp
        if (dsp != null) {
            val conv = dsp.convolutionRate
            var what = if (dsp.eq && conv != null) "equalised and convolved" else if (dsp.eq) "equalised" else "convolved"
            if (conv != null && conv != f.sampleRate) {
                what += ", resampled ${rate(f.sampleRate.toLong())} kHz → ${rate(conv.toLong())} kHz for convolution"
            }
            return "Processed by “${dsp.profile}”: $what"
        }
        val out = f.outputSampleRate
            ?: return "Source format; koan matches the device to the source rate rather than resampling"
        return if (out == f.sampleRate) {
            "Device is running at ${rate(out.toLong())} kHz, the source rate; koan is resampling nothing"
        } else {
            "Device stayed at ${rate(out.toLong())} kHz, so ${rate(f.sampleRate.toLong())} kHz is being resampled to reach it"
        }
    }

    /** "2007 · 11 tracks · 59:10 · FLAC". The codec only earns its place when the whole run shares one. */
    fun groupDetail(group: QueueGroup): String {
        val parts = mutableListOf<String>()
        group.year?.takeIf { it.isNotEmpty() }?.let { parts += it }
        parts += count(group.items.size.toLong(), "track")
        val total = group.items.sumOf { (it.durationMs ?: 0uL).toLong() }
        if (total > 0) parts += clock(total)
        val codecs = group.items.mapNotNull { it.codec }.toSet()
        if (codecs.size == 1) parts += codecs.first().uppercase()
        return parts.joinToString(" · ")
    }

    /** "1,204 tracks · 3:10:05", for the queue's header. */
    fun summary(items: List<QueueItem>): String {
        val total = items.sumOf { (it.durationMs ?: 0uL).toLong() }
        val n = count(items.size.toLong(), "track")
        return if (total > 0) "$n · ${clock(total)}" else n
    }
}
