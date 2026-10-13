package cc.blit.koan.ui.settings

import android.text.format.DateUtils
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.R
import cc.blit.koan.engine.EngineMirror
import cc.blit.koan.ffi.DeviceInfo
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ffi.PlayState
import cc.blit.koan.ui.koanApp
import cc.blit.koan.ui.theme.LocalKoanTokens
import kotlinx.coroutines.launch

/**
 * Play on: a button for Now Playing that opens the device picker. Its icon
 * fills while another device is controlled.
 */
@Composable
fun PlayOnButton(modifier: Modifier = Modifier) {
    val devices by koanApp().mirror.devices.collectAsStateWithLifecycle()
    var open by remember { mutableStateOf(false) }
    IconButton({ open = true }, modifier) {
        Icon(
            painterResource(R.drawable.ic_play_on),
            contentDescription = "Play on",
            tint = if (devices.target != null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
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

    /** Whether `to` (this phone for `null`) can take the music the device in view has. */
    fun canMove(to: DeviceInfo?): Boolean {
        if (to?.id == devices.target || to?.problem != null) return false
        if (controlled != null) {
            if (!controlled.sameLibrary || controlled.state == PlayState.STOPPED || controlled.problem != null) return false
        } else if (queue.items.isEmpty()) {
            return false
        }
        return to?.sameLibrary ?: true
    }

    fun attempt(block: suspend () -> Unit) = scope.launch {
        try {
            block()
        } catch (e: KoanException) {
            notice = e.describe()
        }
    }

    fun move(to: DeviceInfo?) = attempt {
        notice = moved(mirror, to, controlled)
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding()) {
            Text(
                "Play on",
                style = MaterialTheme.typography.titleLarge,
                modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter),
            )
            notice?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = LocalKoanTokens.current.gutter, vertical = 8.dp),
                )
            }
            DeviceRow(
                name = "This phone",
                detail = if (controlled != null) "Paused while you control another device" else null,
                selected = controlled == null,
                canMove = controlled != null && canMove(null),
                onSelect = { attempt { mirror.engine().controlDevice(null) } },
                onMove = { move(null) },
            )
            devices.devices.forEach { device ->
                DeviceRow(
                    name = device.name,
                    detail = detail(device),
                    selected = device.id == devices.target,
                    canMove = canMove(device),
                    // Out of reach with nothing able to wake it: a command would go nowhere.
                    unreachable = device.problem != null || (!device.awake && !device.wakeable),
                    onSelect = { attempt { mirror.engine().controlDevice(device.id) } },
                    onMove = { move(device) },
                    onForget = if (!device.awake) ({ attempt { mirror.engine().forgetDevice(device.id) } }) else null,
                )
            }
            if (devices.devices.isEmpty()) {
                Text(
                    if (connection?.devices == true) {
                        "No other kōan devices. Open kōan on another device signed in to this server, or on this network."
                    } else {
                        "No other kōan devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(LocalKoanTokens.current.gutter),
                )
            }
        }
    }
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
    ListItem(
        headlineContent = { Text(name) },
        supportingContent = detail?.takeIf { it.isNotEmpty() }?.let { { Text(it) } },
        leadingContent = { RadioButton(selected, onClick = onSelect, enabled = !unreachable) },
        trailingContent = when {
            canMove -> ({ TextButton(onMove) { Text("Move here") } })
            onForget != null -> ({ TextButton(onForget) { Text("Forget") } })
            else -> null
        },
        colors = if (unreachable) {
            ListItemDefaults.colors(headlineColor = MaterialTheme.colorScheme.onSurfaceVariant)
        } else {
            ListItemDefaults.colors()
        },
        modifier = Modifier.clickable(enabled = !unreachable, onClick = onSelect),
    )
}

/** What a device is doing, or why it cannot be reached. */
private fun detail(device: DeviceInfo): String {
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
            " · seen " + DateUtils.getRelativeTimeSpanString(it * 1000, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
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
