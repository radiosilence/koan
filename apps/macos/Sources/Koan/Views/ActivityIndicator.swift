import SwiftUI

/// What the app is busy with, in the toolbar.
///
/// Scans, syncs and large queue edits took anywhere up to a minute with nothing
/// to show for it but a small line in the sidebar footer that said "Scanning…"
/// whatever was actually happening. This says which task, and disappears when
/// there is none — a permanently visible idle state is just furniture.
struct ActivityIndicator: View {
    @Environment(ActivityModel.self) private var activity

    var body: some View {
        if let task = activity.current {
            HStack(spacing: 6) {
                if let progress = task.progress {
                    ProgressView(value: progress)
                        .progressViewStyle(.circular)
                        .controlSize(.small)
                } else {
                    ProgressView()
                        .progressViewStyle(.circular)
                        .controlSize(.small)
                }
                VStack(alignment: .leading, spacing: 0) {
                    Text(task.label)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    // What it is on right now — proof it is moving, which a
                    // percentage alone does not give on a slow step.
                    if let detail = task.detail {
                        Text(detail)
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }
                }
                .lineLimit(1)
                .frame(maxWidth: 220, alignment: .leading)

                if let progress = task.progress {
                    Text("\(Int(progress * 100))%")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
                // More than one at once is normal — a sync while a queue add
                // lands — and the count is cheaper than listing them.
                if activity.tasks.count > 1 {
                    Text("+\(activity.tasks.count - 1)")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
            }
            .help(activity.tasks.map(\.label).joined(separator: "\n"))
            .transition(.opacity)
        }
    }
}
