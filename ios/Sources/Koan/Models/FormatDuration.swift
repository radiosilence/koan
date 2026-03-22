import Foundation

func formatDurationMs(_ ms: UInt64) -> String {
    let total = Int(ms / 1000)
    return String(format: "%d:%02d", total / 60, total % 60)
}
