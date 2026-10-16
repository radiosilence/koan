package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import uniffi.koan_ffi.ShelfKind

/** The Recently played shelf (Section.RecentlyPlayed): what you listened to lately, newest first. */
@Composable
fun RecentlyPlayedPage(runtime: KoanRuntime) {
    ShelfPage(
        runtime, Section.RecentlyPlayed, ShelfKind.Recent, "recently played",
        emptyIcon = "clock",
        emptyTitle = "nothing played lately",
        emptyDetail = "What you play shows up here, newest first.",
    )
}
