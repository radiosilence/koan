package cc.blit.koan.state

import android.content.Context
import android.content.SharedPreferences

/**
 * Where a model keeps the little the engine does not: a sort order, a filter
 * set. The port of the `UserDefaults` the Swift models read, and as small.
 *
 * Reads and writes are blocking disk access on Android, so a model is built
 * and read on the engine dispatcher, never Main (StrictMode kills a debug
 * build that does otherwise).
 */
interface KeyValueStore {
    fun getString(key: String): String?
    fun putString(key: String, value: String?)

    /** Held in memory: tests, and a model that wants nothing remembered. */
    class Memory : KeyValueStore {
        private val values = HashMap<String, String>()
        override fun getString(key: String): String? = synchronized(values) { values[key] }
        override fun putString(key: String, value: String?) = synchronized(values) {
            if (value == null) values.remove(key) else values[key] = value
            Unit
        }
    }

    /** `SharedPreferences`, opened on first use. */
    class Prefs(private val context: Context, private val name: String = "koan-ui") : KeyValueStore {
        private val prefs: SharedPreferences by lazy { context.getSharedPreferences(name, Context.MODE_PRIVATE) }
        override fun getString(key: String): String? = prefs.getString(key, null)
        override fun putString(key: String, value: String?) {
            prefs.edit().apply { if (value == null) remove(key) else putString(key, value) }.apply()
        }
    }
}
