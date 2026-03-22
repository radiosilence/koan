import SwiftUI

struct QueueItemRow: View {
    let entry: QueueEntry

    var body: some View {
        HStack(spacing: 12) {
            if entry.isCurrent {
                Image(systemName: "speaker.wave.2.fill")
                    .font(.caption)
                    .foregroundStyle(Color.accentColor)
                    .frame(width: 20)
            } else {
                Text(entry.trackNumber.map { "\($0)" } ?? "")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .frame(width: 20)
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(entry.title)
                    .font(.subheadline)
                    .fontWeight(entry.isCurrent ? .semibold : .regular)
                    .lineLimit(1)

                Text("\(entry.artist) — \(entry.album)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            Text(entry.durationFormatted)
                .font(.caption)
                .foregroundStyle(.tertiary)
                .monospacedDigit()
        }
        .padding(.vertical, 2)
        .listRowBackground(entry.isCurrent ? Color.accentColor.opacity(0.1) : nil)
    }
}
