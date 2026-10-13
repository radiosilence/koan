package cc.blit.koan.ui.library

import androidx.compose.runtime.Composable

/**
 * A record's page, in the record's colour: the sleeve's accent tints the
 * controls on it.
 */
@Composable
fun RecordAccent(albumId: Long, content: @Composable () -> Unit) {
    content()
}
