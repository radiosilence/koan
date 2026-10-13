package cc.blit.koan.ui.pages

import androidx.compose.runtime.Composable
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.nav.Section
import uniffi.koan_ffi.ShelfKind

/** The On this device shelf (Section.OnDevice): what plays without a network, downloaded or local. */
@Composable
fun OnDevicePage(runtime: KoanRuntime) {
    ShelfPage(
        runtime, Section.OnDevice, ShelfKind.Downloaded, "downloaded",
        emptyIcon = "drive",
        emptyTitle = "nothing downloaded yet",
        emptyDetail = "Records you play or download from your server appear here, with how much of each is on this device.",
    )
}
