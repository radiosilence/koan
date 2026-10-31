package cc.blit.koan.ui.library

import cc.blit.koan.ffi.RunRole

/** A grouped list's rows as the engine marked them (`helpers::record_runs`): a record's run, or a row on its own. */
sealed interface RunPiece<T> {
    /** A row on no record, or a single, which is its whole record: `single` says which. */
    data class Alone<T>(val row: T, val single: Boolean) : RunPiece<T>

    /** A record's rows, the first the one that opens the run. */
    data class Run<T>(val rows: List<T>) : RunPiece<T>
}

/**
 * The one walk over the engine's run marks, for every list that groups by
 * record. A row that continues a run nothing opened stands alone.
 */
fun <T> runPieces(rows: List<T>, role: (T) -> RunRole): List<RunPiece<T>> {
    val pieces = ArrayList<RunPiece<T>>()
    var index = 0
    while (index < rows.size) {
        val first = rows[index]
        when (role(first)) {
            RunRole.ALONE, RunRole.CONTINUES -> pieces += RunPiece.Alone(first, single = false)
            RunRole.SINGLE -> pieces += RunPiece.Alone(first, single = true)
            RunRole.OPENS -> {
                var end = index + 1
                while (end < rows.size && role(rows[end]) == RunRole.CONTINUES) end++
                pieces += RunPiece.Run(rows.subList(index, end).toList())
                index = end
                continue
            }
        }
        index++
    }
    return pieces
}
