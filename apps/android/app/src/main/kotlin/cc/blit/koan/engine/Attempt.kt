package cc.blit.koan.engine

import kotlinx.coroutines.CancellationException

/**
 * `runCatching` for engine calls: a failure becomes a `Result`, but
 * cancellation goes on up, so a coroutine told to stop does stop rather than
 * carry on with a failure in hand. Errors are not caught either.
 */
inline fun <T> attempt(block: () -> T): Result<T> = try {
    Result.success(block())
} catch (e: CancellationException) {
    throw e
} catch (e: Exception) {
    Result.failure(e)
}
