import SwiftUI

struct SeekBar: View {
    let positionMs: UInt64
    let durationMs: UInt64?
    let onSeek: (Int64) -> Void

    @State private var isDragging = false
    @State private var dragProgress: Double = 0

    private var progress: Double {
        guard let dur = durationMs, dur > 0 else { return 0 }
        return isDragging ? dragProgress : Double(positionMs) / Double(dur)
    }

    private var positionLabel: String {
        let ms = isDragging ? UInt64(dragProgress * Double(durationMs ?? 0)) : positionMs
        return formatDurationMs(ms)
    }

    var body: some View {
        VStack(spacing: 4) {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(.quaternary)
                        .frame(height: 4)

                    Capsule()
                        .fill(.primary)
                        .frame(width: max(0, geo.size.width * progress), height: 4)

                    Circle()
                        .fill(.primary)
                        .frame(width: isDragging ? 16 : 8, height: isDragging ? 16 : 8)
                        .offset(x: max(0, geo.size.width * progress - (isDragging ? 8 : 4)))
                        .animation(.easeOut(duration: 0.15), value: isDragging)
                }
                .frame(height: 16)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            isDragging = true
                            dragProgress = max(0, min(1, value.location.x / geo.size.width))
                        }
                        .onEnded { value in
                            let p = max(0, min(1, value.location.x / geo.size.width))
                            if let dur = durationMs {
                                onSeek(Int64(p * Double(dur)))
                            }
                            isDragging = false
                        }
                )
            }
            .frame(height: 16)

            HStack {
                Text(positionLabel)
                Spacer()
                Text(formatDurationMs(durationMs ?? 0))
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .monospacedDigit()
        }
    }
}
