package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanProcess
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.service.BitPerfectStatus
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * Settings (the Settings tab's root).
 *
 * Placeholder: the output route's bit-perfect status, which nothing else shows
 * yet. The panes (Library, Server, Playback, Devices, Appearance) are built from
 * `runtime.settings`, grown from `SettingsView.swift`'s panes.
 */
@Composable
fun SettingsPage(runtime: KoanRuntime) {
    val bitPerfect by KoanProcess.bitPerfect.collectAsStateWithLifecycle()
    Column(Modifier.fillMaxSize().padding(KoanSpace.page), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        KoanText("settings", KoanRole.Title, KoanTone.Strong)
        KoanText(
            when (val status = bitPerfect) {
                BitPerfectStatus.Exclusive -> "output: bit-perfect (exclusive)"
                BitPerfectStatus.MixerBitPerfect -> "output: bit-perfect (mixer)"
                is BitPerfectStatus.No -> "output: not bit-perfect (${status.reason})"
            },
            KoanRole.Meta, KoanTone.Muted, maxLines = 3,
        )
    }
}
