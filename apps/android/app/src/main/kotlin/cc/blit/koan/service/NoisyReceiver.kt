package cc.blit.koan.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioManager

/**
 * `ACTION_AUDIO_BECOMING_NOISY`: headphones unplugged, a Bluetooth sink gone.
 * Pause rather than play on through the speaker. Registered only while music
 * is playing, so a paused service does not wake for it.
 */
class NoisyReceiver(private val context: Context, private val onNoisy: () -> Unit) {
    private var registered = false

    private val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) onNoisy()
        }
    }

    fun setActive(active: Boolean) {
        if (active == registered) return
        if (active) {
            context.registerReceiver(receiver, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY))
        } else {
            context.unregisterReceiver(receiver)
        }
        registered = active
    }
}
