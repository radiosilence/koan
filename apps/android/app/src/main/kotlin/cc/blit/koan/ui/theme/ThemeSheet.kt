package cc.blit.koan.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Sleeve colours for the sheet's accent: none, then a red, a navy, a muddy brown and a neon green. */
private val sleeves = listOf<Pair<String, Color?>>(
    "Mint" to null,
    "Red" to Color(0xFFB0302A),
    "Navy" to Color(0xFF1B2A5C),
    "Mud" to Color(0xFF6B5A3A),
    "Neon" to Color(0xFF39FF14),
)

/**
 * Every component of the kōan theme on one page: what a change to the theme
 * is checked against before any screen is. The look, the appearance and the
 * record's accent can be switched from its head.
 */
@Composable
fun ThemeSheet() {
    var koan by remember { mutableStateOf(true) }
    var dark by remember { mutableStateOf(true) }
    var sleeve by remember { mutableIntStateOf(0) }
    KoanTheme(koan = koan, dark = dark, accent = sleeves[sleeve].second?.let(KoanAccent::of)) {
        Column(
            Modifier
                .fillMaxSize()
                .background(if (koan) KoanTheme.colors.bg else androidx.compose.material3.MaterialTheme.colorScheme.background)
                .safeDrawingPadding()
                .verticalScroll(rememberScrollState())
                .padding(KoanSpace.page),
            verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
        ) {
            KoanSegmented(listOf("Kōan" to true, "System" to false), koan, { koan = it })
            KoanSegmented(listOf("Dark" to true, "Light" to false), dark, { dark = it })
            KoanSegmented(sleeves.mapIndexed { i, (name, _) -> name to i }, sleeve, { sleeve = it })
            Sheet()
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Sheet() {
    var on by remember { mutableStateOf(true) }
    var off by remember { mutableStateOf(false) }
    var segment by remember { mutableIntStateOf(0) }
    var level by remember { mutableFloatStateOf(0.38f) }
    var name by remember { mutableStateOf("Sunday morning") }
    var output by remember { mutableStateOf("Built-in speaker") }
    var tab by remember { mutableStateOf("Library") }
    var place by remember { mutableStateOf("Albums") }

    KoanText("kōan", role = KoanType.Display, tone = KoanTone.Strong)
    KoanText("Albums", role = KoanType.Title, tone = KoanTone.Strong, appText = true)
    KoanText("Low Tide Arcade", role = KoanType.TitleSmall, tone = KoanTone.Strong)
    KoanText("Body text, in ink.")
    KoanText("Secondary text, muted.", role = KoanType.Meta, tone = KoanTone.Muted)
    KoanText("The record's accent, as text.", tone = KoanTone.Accent)
    KoanText("Something went wrong.", role = KoanType.Fine, tone = KoanTone.Bad)

    KoanSectionHeader("Tokens")
    val c = KoanTheme.colors
    FlowRow(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalArrangement = Arrangement.spacedBy(KoanSpace.s)) {
        for ((label, colour) in listOf(
            "bg" to c.bg, "surface" to c.surface, "rule" to c.rule, "hover" to c.hover, "ink" to c.ink,
            "strong" to c.strong, "muted" to c.muted, "bad" to c.bad, "accent" to KoanTheme.look.accent,
        )) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Box(Modifier.size(32.dp).background(colour).koanRule(KoanEdge.Bottom))
                KoanText(label, role = KoanType.Fine, tone = KoanTone.Muted)
            }
        }
    }

    KoanSectionHeader("Buttons")
    FlowRow(
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.m),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.s),
        itemVerticalAlignment = Alignment.CenterVertically,
    ) {
        KoanButton("Play", {}, kind = KoanButtonKind.Prominent, icon = KoanIcons.play)
        KoanButton("Shuffle", {}, icon = KoanIcons.shuffle)
        KoanButton("Copy", {}, kind = KoanButtonKind.Bordered)
        KoanButton("Find your token", {}, kind = KoanButtonKind.Link)
        KoanButton("Clear", {}, kind = KoanButtonKind.Text)
        KoanButton("Sleep", {}, kind = KoanButtonKind.Icon, icon = KoanIcons.sleep)
        KoanButton("Pause", {}, kind = KoanButtonKind.IconOutlined, icon = KoanIcons.pause)
        KoanButton("Revoke", {}, kind = KoanButtonKind.Compact, destructive = true)
        KoanButton("Disabled", {}, enabled = false)
    }

    KoanSectionHeader("Tabs")
    KoanTabBar {
        for (title in listOf("Queue", "Library", "Search", "Settings")) {
            KoanTabItem(title, selected = title == tab, onClick = { tab = title }, icon = KoanIcons.library)
        }
    }

    KoanSectionHeader("Navigation")
    Column {
        for ((title, icon) in listOf("Albums" to KoanIcons.album, "Artists" to KoanIcons.artist, "Playlists" to KoanIcons.playlist)) {
            KoanNavRow(title, icon, selected = title == place, onClick = { place = title })
        }
    }

    KoanSectionHeader("Toggles")
    KoanToggle("Show icons", on, { on = it })
    KoanToggle("Gapless", off, { off = it })

    KoanSectionHeader("Segmented")
    KoanSegmented(listOf("All" to 0, "Favourites" to 1, "Lossless" to 2), segment, { segment = it })

    KoanSectionHeader("Slider")
    KoanSlider(
        level, { level = it },
        low = { KoanText("1:24", role = KoanType.Fine, tone = KoanTone.Muted) },
        high = { KoanText("3:41", role = KoanType.Fine, tone = KoanTone.Muted) },
    )

    KoanSectionHeader("Rows")
    Column {
        for ((title, time) in listOf("Shoreline Pinball" to "3:41", "Coin Return" to "4:02", "Neon Breakwater" to "5:17")) {
            Row(
                Modifier.fillMaxWidth().koanRow(selected = title == "Coin Return").padding(horizontal = KoanSpace.s, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.weight(1f)) { KoanText(title, tone = if (title == "Neon Breakwater") KoanTone.Accent else KoanTone.Ink, maxLines = 1) }
                KoanText(time, role = KoanType.Meta, tone = KoanTone.Muted)
            }
        }
    }

    KoanSectionHeader("Form")
    KoanFormRow("Name") { KoanField(name, { name = it }, Modifier.fillMaxWidth(), placeholder = "Name") }
    KoanFormRow("Output") {
        KoanPicker(listOf("Built-in speaker" to "Built-in speaker", "USB DAC" to "USB DAC"), output, { output = it }, keepsCase = true)
    }
    KoanFormRow("Version") { KoanText("0.61.13", role = KoanType.Control, tone = KoanTone.Muted) }
    KoanText(
        "A footer that explains the section, long enough to wrap onto a second line.",
        role = KoanType.Fine,
        tone = KoanTone.Muted,
    )

    KoanSectionHeader("Badges and chips")
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        KoanBadge("FLAC 24/96")
        KoanBadge("Bit-perfect", accent = true)
        Box(Modifier.koanChip()) { KoanText("Living room", role = KoanType.Control) }
    }

    KoanSectionHeader("Surface")
    Box(Modifier.fillMaxWidth().koanSurface(KoanSurface.Surface).padding(KoanSpace.m)) {
        KoanText("search", tone = KoanTone.Muted)
    }
    KoanDivider()

    KoanSectionHeader("Icons")
    FlowRow(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalArrangement = Arrangement.spacedBy(KoanSpace.m)) {
        for ((glyph, icon) in KoanIcons.all) {
            KoanIcon(icon, contentDescription = glyph, size = 17.sp)
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(KoanSpace.m), verticalAlignment = Alignment.CenterVertically) {
        for (size in listOf(12, 15, 20, 28)) KoanIcon(KoanIcons.favourited, contentDescription = null, size = size.sp)
        KoanIcon(KoanIcons.selecting, contentDescription = null, size = 20.sp)
        KoanIcon(KoanIcons.selecting, contentDescription = null, size = 20.sp, fill = KoanTheme.look.accent)
    }

    KoanSectionHeader("Empty")
    Box(Modifier.height(180.dp)) {
        KoanUnavailable("No playlists", KoanIcons.playlist, "Playlists you make, or sync from a server, show here.")
    }
    Spacer(Modifier.width(1.dp).height(KoanSpace.xxl))
}

@Preview(widthDp = 400, heightDp = 2400)
@Composable
private fun ThemeSheetPreview() = ThemeSheet()
