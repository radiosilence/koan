package cc.blit.koan.ui.theme

import android.content.Context
import androidx.core.content.edit
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * How much kōan spends on looking like itself, as the Apple apps' `Graphics`:
 * each step down removes something that costs while the music plays, the
 * wash's drift first. The values are the Apple apps' raw values. Their Bare
 * step takes the platform's own glass away, which Android does not draw, so
 * here it is Plain.
 */
enum class Graphics(val raw: Int, val label: String, val detail: String) {
    Plain(0, "Plain", "No colour behind the pages."),
    Reduced(1, "Reduced", "The record's colour behind the pages, held still, which costs the same as no colour at all. Only the drift is expensive."),
    Full(2, "Full", "The colour drifts while something is playing, for a little more battery for as long as the music runs."),
    ;

    /** Whether the wash is drawn at all. */
    val showsWash get() = this != Plain

    /** Whether the wash drifts while something plays. */
    val drifts get() = this == Full

    companion object {
        /** The step a stored value is: Bare, from another platform's value, is Plain here. */
        fun of(raw: Int) = entries.firstOrNull { it.raw == raw } ?: if (raw == 3) Plain else Full
    }
}

/**
 * The graphics level, kept on this device: how much it draws is its own
 * business, as iOS keeps it beside the app's other view state rather than
 * in the configuration the CLI shares.
 */
class GraphicsSetting(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val _level = MutableStateFlow(Graphics.of(prefs.getInt(KEY, Graphics.Full.raw)))
    val level: StateFlow<Graphics> = _level.asStateFlow()

    fun set(level: Graphics) {
        prefs.edit { putInt(KEY, level.raw) }
        _level.value = level
    }

    private companion object {
        const val PREFS = "appearance"
        const val KEY = "graphics"
    }
}
