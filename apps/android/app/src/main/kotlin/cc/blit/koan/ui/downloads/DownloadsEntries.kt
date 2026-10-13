package cc.blit.koan.ui.downloads

import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.navigation.nonce
import kotlinx.serialization.Serializable

@Serializable
data class DownloadsRoute(val nonce: String = nonce()) : NavKey

fun EntryProviderScope<NavKey>.downloadsEntries() {
    entry<DownloadsRoute> { StubPage("Downloads") }
}
