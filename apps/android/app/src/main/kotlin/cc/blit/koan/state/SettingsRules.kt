package cc.blit.koan.state

import java.util.Locale

/**
 * What a settings field may hold, decided in one place so a page and a test
 * agree. Each rule answers with the value to store, or null when what was
 * typed is not one; the page says so and writes nothing.
 *
 * The rules follow the engine's own (`config::parse_size_bytes`, the device
 * addresses `nearby` dials), so a value accepted here is a value the engine
 * reads back the same.
 */
object SettingsRules {
    const val MIN_WORKERS = 1
    const val MAX_WORKERS = 16
    const val MIN_PRE_AMP = -15.0
    const val MAX_PRE_AMP = 15.0
    const val PRE_AMP_STEP = 0.5

    /** The intervals the automatic sync offers, in minutes; 0 is "at startup only". */
    val SyncIntervals: List<Long> = listOf(0, 15, 60, 360, 1440)

    private val sizePattern = Regex("""^(\d+(?:\.\d+)?|\.\d+)\s*(B|KB|K|MB|M|GB|G|TB|T)?$""", RegexOption.IGNORE_CASE)

    /**
     * The cache limit as stored: blank for no limit, otherwise a positive number
     * with an optional unit (`50GB`, `1.5 TB`, `500m`), the unit upper-cased.
     */
    fun cacheLimit(typed: String): String? {
        val text = typed.trim()
        if (text.isEmpty()) return ""
        val match = sizePattern.matchEntire(text) ?: return null
        val number = match.groupValues[1]
        if (number.toDoubleOrNull()?.let { it > 0.0 } != true) return null
        val unit = match.groupValues[2].uppercase(Locale.ROOT)
        return number + unit
    }

    /**
     * A device to dial by address: `host`, `host:port`, `[v6]` or `[v6]:port`,
     * with the port in range. A URL, a path or anything with a space is not one.
     */
    fun address(typed: String): String? {
        val text = typed.trim()
        if (text.isEmpty() || text.any { it.isWhitespace() || it == '/' || it == '@' }) return null
        val host: String
        val port: String?
        when {
            text.startsWith("[") -> {
                val close = text.indexOf(']')
                if (close < 0) return null
                host = text.substring(1, close)
                val rest = text.substring(close + 1)
                port = when {
                    rest.isEmpty() -> null
                    rest.startsWith(":") -> rest.substring(1)
                    else -> return null
                }
                if (host.isEmpty() || !host.all { it.isLetterOrDigit() || it == ':' || it == '.' } || ':' !in host) return null
            }
            text.count { it == ':' } > 1 -> {
                // A bare IPv6 address: no port can be told from the groups.
                if (!text.all { it.isDigit() || it in 'a'..'f' || it in 'A'..'F' || it == ':' || it == '.' }) return null
                return text
            }
            text.contains(':') -> {
                host = text.substringBefore(':')
                port = text.substringAfter(':')
            }
            else -> {
                host = text
                port = null
            }
        }
        if (host.isEmpty() || !text.startsWith("[") && !host.all { it.isLetterOrDigit() || it == '.' || it == '-' || it == '_' }) return null
        if (port != null) {
            val n = port.toIntOrNull() ?: return null
            if (n !in 1..65535) return null
        }
        return text
    }

    fun workers(n: Int): UInt = n.coerceIn(MIN_WORKERS, MAX_WORKERS).toUInt()

    /** Pre-amp in whole half-decibels within the engine's range. */
    fun preAmp(db: Double): Double {
        val stepped = Math.round(db / PRE_AMP_STEP) * PRE_AMP_STEP
        return stepped.coerceIn(MIN_PRE_AMP, MAX_PRE_AMP)
    }

    fun preAmpText(db: Double): String = String.format(Locale.ROOT, "%.1f db", db)

    /** The interval's words, for the picker. */
    fun intervalLabel(minutes: Long): String = when (minutes) {
        0L -> "startup only"
        15L -> "15 minutes"
        60L -> "hour"
        360L -> "6 hours"
        1440L -> "day"
        else -> "$minutes minutes"
    }

    /** A byte count as the apps print it: `1.5 GB`, binary multiples. */
    fun bytes(count: Long): String {
        if (count < 1024) return "$count B"
        val units = listOf("KB", "MB", "GB", "TB")
        var value = count.toDouble()
        var unit = -1
        while (value >= 1024 && unit < units.lastIndex) {
            value /= 1024
            unit++
        }
        return if (value >= 100) String.format(Locale.ROOT, "%.0f %s", value, units[unit])
        else String.format(Locale.ROOT, "%.1f %s", value, units[unit])
    }
}

/**
 * The code another device shows while it waits to be signed in
 * (`XXXX-XXXX`, Crockford's alphabet). Typed by hand, so it is forgiven its
 * case, its spaces and its missing dash.
 */
object PairCode {
    /** The code in its canonical form, or null when what was typed cannot be one. */
    fun normalise(typed: String): String? {
        val letters = typed.filter { it.isLetterOrDigit() }.uppercase(Locale.ROOT)
        if (letters.length != 8) return null
        return letters.substring(0, 4) + "-" + letters.substring(4)
    }
}
