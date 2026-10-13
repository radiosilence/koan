package cc.blit.koan.ui.settings

import android.text.format.DateUtils
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.BottomSheetDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcon
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTheme
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import kotlinx.coroutines.launch

/**
 * Play on: a button for Now Playing that opens the device picker. Its glyph
 * takes the accent while another device is controlled.
 */
@Composable
fun PlayOnButton(modifier: Modifier = Modifier) {
    val devices by koanApp().mirror.devices.collectAsStateWithLifecycle()
    var open by remember { mutableStateOf(false) }
    KoanButton({ open = true }, modifier, kind = KoanButtonKind.Icon) {
        KoanIcon(
            KoanIcons.devices,
            contentDescription = "Play on",
            color = KoanTheme.color(if (devices.target != null) KoanTone.Accent else KoanTone.Ink),
        )
    }
    if (open) PlayOnSheet(onDismiss = { open = false })
}

/**
 * Which kōan the transport, the queue and Now Playing show and command: this
 * phone, or another of the account's or the network's. Picking another pauses
 * this one and turns everything into that device's until another is picked.
 * Nothing moves until "Move here", which sends what the device controlled is
 * playing to that row's device.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlayOnSheet(onDismiss: () -> Unit) {
    val mirror = koanApp().mirror
    val devices by mirror.devices.collectAsStateWithLifecycle()
    val connection by mirror.connection.collectAsStateWithLifecycle()
    val queue by mirror.queue.collectAsStateWithLifecycle()
    val scope = rememberCoroutineScope()
    var notice by remember { mutableStateOf<String?>(null) }
    val controlled = devices.devices.firstOrNull { it.id == devices.target }
    val hasQueue = queue.items.isNotEmpty()

    fun attempt(block: suspend () -> Unit) = scope.launch {
        try {
            block()
        } catch (e: KoanException) {
            notice = e.describe()
        }
    }

    val on = KoanTheme.isOn
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        shape = if (on) RectangleShape else BottomSheetDefaults.ExpandedShape,
        containerColor = if (on) KoanTheme.colors.bg else BottomSheetDefaults.ContainerColor,
        tonalElevation = 0.dp,
    ) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = KoanSpace.l)) {
            KoanText("Play on", inset, role = KoanType.TitleSmall, tone = KoanTone.Strong, appText = true)
            notice?.let { KoanText(it, inset.padding(vertical = KoanSpace.s), role = KoanType.Fine, tone = KoanTone.Muted) }
            DeviceRow(
                name = "This phone",
                detail = if (controlled != null) "Paused while you control another device" else null,
                selected = controlled == null,
                canMove = controlled != null && canMoveMusic(null, devices.target, controlled, hasQueue),
                onSelect = { attempt { mirror.engine().controlDevice(null) } },
                onMove = { attempt { notice = moved(mirror, null, controlled) } },
            )
            devices.devices.forEach { device ->
                DeviceRow(
                    name = device.name,
                    detail = detail(device),
                    selected = device.id == devices.target,
                    canMove = canMoveMusic(device, devices.target, controlled, hasQueue),
                    // Out of reach with nothing able to wake it: a command would go nowhere.
                    unreachable = device.problem != null || (!device.awake && !device.wakeable),
                    onSelect = { attempt { mirror.engine().controlDevice(device.id) } },
                    onMove = { attempt { notice = moved(mirror, device, controlled) } },
                    onForget = if (!device.awake) ({ attempt { mirror.engine().forgetDevice(device.id) } }) else null,
                )
            }
            if (devices.devices.isEmpty()) {
                KoanText(
                    if (connection?.devices == true) {
                        "No other kōan devices. Open kōan on another device signed in to this server, or on this network."
                    } else {
                        "No other kōan devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
                    },
                    inset.padding(vertical = KoanSpace.s),
                    role = KoanType.Fine,
                    tone = KoanTone.Muted,
                )
            }
        }
    }
}

/**
 * Whether `to` (this phone for `null`) can take the music the device in view
 * has: both must play from the same library, and there must be something to
 * move.
 */
internal fun canMoveMusic(to: DeviceInfo?, target: String?, controlled: DeviceInfo?, hasQueue: Boolean): Boolean {
    if (to?.id == target || to?.problem != null) return false
    if (controlled != null) {
        if (!controlled.sameLibrary || controlled.state == PlayState.STOPPED || controlled.problem != null) return false
    } else if (!hasQueue) {
        return false
    }
    return to?.sameLibrary ?: true
}

@Composable
private fun DeviceRow(
    name: String,
    detail: String?,
    selected: Boolean,
    canMove: Boolean,
    onSelect: () -> Unit,
    onMove: () -> Unit,
    unreachable: Boolean = false,
    onForget: (() -> Unit)? = null,
) {
    Row(inset.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
        KoanButton(onSelect, Modifier.weight(1f), kind = KoanButtonKind.Card, enabled = !unreachable) {
            Row(Modifier.padding(vertical = KoanSpace.s), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(28.dp)) {
                    if (selected) KoanIcon(KoanIcons.check, contentDescription = "Controlling", color = KoanTheme.color(KoanTone.Accent))
                }
                Column {
                    KoanText(name, tone = if (selected) KoanTone.Accent else KoanTone.Ink)
                    detail?.takeIf { it.isNotEmpty() }?.let { KoanText(it, role = KoanType.Fine, tone = KoanTone.Muted) }
                }
            }
        }
        when {
            canMove -> KoanButton("Move here", onMove, kind = KoanButtonKind.Compact)
            onForget != null -> KoanButton("Forget", onForget, kind = KoanButtonKind.Compact, destructive = true)
        }
    }
}

/** What a device is doing, or why it cannot be reached. */
internal fun detail(device: DeviceInfo, now: Long = System.currentTimeMillis()): String {
    device.problem?.let { return it }
    device.waking?.let { stage ->
        return when (stage) {
            "network" -> "Waking… trying it on this network"
            "push" -> "Waking… sent a wake through your server"
            else -> "Waking… tap the notification on ${device.name}"
        }
    }
    if (!device.awake && !device.asleep) return "Reconnecting…"
    if (!device.awake) {
        val seen = device.lastSeen?.let {
            " · seen " + DateUtils.getRelativeTimeSpanString(it * 1000, now, DateUtils.MINUTE_IN_MILLIS)
        } ?: ""
        device.wakeFailed?.let { return "$it Asleep$seen." }
        if (device.wakeable) return "Asleep$seen. Choosing it wakes it; music sent here arrives as a notification to tap."
        return if (device.platform == "macos") "Not running$seen. It can't be woken from here." else "Asleep$seen"
    }
    val playing = listOfNotNull(device.title, device.artist).joinToString(" — ")
    val library = if (device.sameLibrary) "" else " · different library"
    val state = when (device.state) {
        PlayState.PLAYING -> "Playing $playing"
        PlayState.PAUSED -> "Paused · $playing"
        PlayState.STOPPED -> "Idle"
    }
    val owner = device.owner?.let { "Shared by $it · " } ?: ""
    return owner + state + library
}

/** Move the music to `to` (this phone for `null`), and say what came of it when it did not simply arrive. */
private suspend fun moved(mirror: EngineMirror, to: DeviceInfo?, from: DeviceInfo?): String? {
    val name = to?.name ?: "This phone"
    val source = from?.name ?: "The other device"
    val moved = mirror.engine().moveMusic(to?.id)
    val left = moved.leftOut.toInt()
    return when {
        moved.error != null ->
            if (to == null) "$source did not send the music here: ${moved.error}" else "$name did not take the music: ${moved.error}"
        moved.queued ->
            if (to == null) {
                "$source is asleep. It sends the music here when it wakes."
            } else {
                "$name is asleep. It takes the music when it wakes; until then the music is paused where it was."
            }
        !moved.started ->
            "Sent to $name, which has not started it yet. It may when it wakes; until then the music is paused where it was."
        left == 1 -> "1 track only on this device stayed behind"
        left > 1 -> "$left tracks only on this device stayed behind"
        else -> null
    }
}
