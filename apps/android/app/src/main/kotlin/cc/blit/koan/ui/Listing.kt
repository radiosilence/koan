@file:OptIn(ExperimentalCoroutinesApi::class)

package cc.blit.koan.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import cc.blit.koan.engine.attempt
import cc.blit.koan.engine.describe
import cc.blit.koan.ffi.KoanException
import cc.blit.koan.ui.theme.KoanButton
import cc.blit.koan.ui.theme.KoanButtonKind
import cc.blit.koan.ui.theme.KoanIcons
import cc.blit.koan.ui.theme.KoanSpace
import cc.blit.koan.ui.theme.KoanText
import cc.blit.koan.ui.theme.KoanTone
import cc.blit.koan.ui.theme.KoanType
import cc.blit.koan.ui.theme.KoanUnavailable
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.mapLatest
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update

/** Something a page reads from the engine: still reading, read, or the read failed and why. */
sealed interface Read<out T> {
    data object Reading : Read<Nothing>

    /** `stale` is why the latest read failed, where this is what an earlier one read. */
    data class Done<T>(val value: T, val stale: String? = null) : Read<T>

    data class Failed(val reason: String) : Read<Nothing>
}

/** What has been read, or `null` while reading or after a failure. */
val <T> Read<T>.value get() = (this as? Read.Done)?.value

/** `block`'s answer as a `Read`: a failure is kept, with the engine's words for it, never taken for an empty answer. */
inline fun <T> read(block: () -> T): Read<T> = attempt(block).fold(
    { Read.Done(it) },
    { Read.Failed((it as? KoanException)?.describe() ?: it.message ?: it.toString()) },
)

/**
 * A listing read again whenever `keys` moves, and when asked to after a
 * failed read. A new read replaces the last one's answer only when it lands,
 * so a page keeps what it shows meanwhile. A read that fails after one that
 * did not keeps the earlier answer, marked stale with the failure, so the
 * page keeps its content and says what went wrong beside it.
 */
class Listing<K, T>(scope: CoroutineScope, keys: Flow<K>, fetch: suspend (K) -> T) {
    private val retries = MutableStateFlow(0)
    private var last: Read.Done<T>? = null

    val state: StateFlow<Read<T>> = combine(keys, retries) { key, _ -> key }
        .mapLatest { key ->
            when (val r = read { fetch(key) }) {
                is Read.Done -> r.also { last = it }
                is Read.Failed -> last?.copy(stale = r.reason) ?: r
                Read.Reading -> r
            }
        }
        .stateIn(scope, SharingStarted.WhileSubscribed(5_000), Read.Reading)

    fun retry() = retries.update { it + 1 }
}

/**
 * What has been read, for a page to draw; `null` while reading, and after a
 * failure, which this says in the page's place with a way to try again.
 */
@Composable
fun <T> Read<T>.shown(what: String, retry: () -> Unit): T? {
    when (this) {
        Read.Reading -> Unit
        is Read.Done -> return value
        is Read.Failed -> KoanUnavailable("Couldn't read $what", KoanIcons.warningFilled, reason) {
            KoanButton("Try Again", retry)
        }
    }
    return null
}

/** Why the latest read failed, over content an earlier read left, with a way to try again; nothing when the content is current. */
@Composable
fun Read<*>.StaleNotice(what: String, retry: () -> Unit) {
    val stale = (this as? Read.Done)?.stale ?: return
    Row(
        Modifier.fillMaxWidth().padding(horizontal = KoanSpace.l, vertical = KoanSpace.xs),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(KoanSpace.s),
    ) {
        KoanText("Couldn't read $what again: $stale", Modifier.weight(1f), role = KoanType.Fine, tone = KoanTone.Bad, maxLines = 2)
        KoanButton("Try Again", retry, kind = KoanButtonKind.Link)
    }
}
