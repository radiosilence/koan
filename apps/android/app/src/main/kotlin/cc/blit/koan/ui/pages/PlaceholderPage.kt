package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * What a page is until its phase 3b worker builds it: its title, and the
 * state it is to be built from. Delete the call, not the file, when the page
 * lands: the signature in `ui/AppPages.kt` is the contract.
 */
@Composable
fun PlaceholderPage(title: String, builtFrom: String, modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxSize().padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanText(title, KoanRole.Title, KoanTone.Strong)
        KoanText(builtFrom, KoanRole.Meta, KoanTone.Muted, maxLines = 6)
    }
}
