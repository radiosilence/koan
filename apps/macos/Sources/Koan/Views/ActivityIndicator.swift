import SwiftUI

/// What the app is busy with, stacked at the foot of the sidebar.
///
/// Scans, syncs and large queue edits take anywhere up to a minute. This was a
/// pill in the toolbar, which had room for one task and truncated its label; the
/// sidebar has the width for a real label and grows downwards when more than one
/// thing is running, which is normal — a sync while a queue add lands.
///
/// Shows nothing at all when idle. A permanent empty state is furniture.
struct ActivityList: View {
    @Environment(ActivityModel.self) private var activity

    var body: some View {
        if !activity.tasks.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(activity.tasks) { task in
                    ActivityRow(task: task)
                }
            }
            .padding(.bottom, 4)
            .transition(.opacity)
            .animation(.easeOut(duration: 0.15), value: activity.tasks.count)
        }
    }
}

private struct ActivityRow: View {
    let task: ActivityModel.Task

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(task.label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                if let done = task.done, let total = task.total, total > 0 {
                    Text("\(done.formatted(.number)) / \(total.formatted(.number))")
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                } else if let done = task.done {
                    Text(done.formatted(.number))
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
            }

            // Determinate where the engine can say, a barber's pole where it
            // cannot — both are the same height, so the row does not jump when
            // a total arrives partway through.
            ProgressView(value: task.progress)
                .progressViewStyle(.linear)
                .controlSize(.small)

            // What it is on right now. Proof it is moving, which a percentage
            // alone does not give during a slow step.
            if let detail = task.detail {
                Text(detail)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .help(task.detail ?? task.label)
    }
}
