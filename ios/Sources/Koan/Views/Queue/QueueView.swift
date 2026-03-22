import SwiftUI

struct QueueView: View {
    @Environment(PlaybackManager.self) private var playback
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(playback.queue) { entry in
                    QueueItemRow(entry: entry)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            playback.playItem(queueItemId: entry.queueItemId)
                        }
                }
                .onDelete { indexSet in
                    let ids = indexSet.map { playback.queue[$0].queueItemId }
                    playback.removeFromQueue(queueItemIds: ids)
                }
                .onMove { source, destination in
                    handleMove(from: source, to: destination)
                }
            }
            .listStyle(.plain)
            .navigationTitle("Queue")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                #if os(iOS)
                ToolbarItem(placement: .topBarLeading) {
                    Button("Clear") { playback.clearQueue() }
                        .disabled(playback.queue.isEmpty)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
                #else
                ToolbarItem(placement: .cancellationAction) {
                    Button("Clear") { playback.clearQueue() }
                        .disabled(playback.queue.isEmpty)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
                #endif
            }
        }
        .onAppear { playback.refreshQueue() }
    }

    private func handleMove(from source: IndexSet, to destination: Int) {
        let queue = playback.queue
        guard !queue.isEmpty else { return }
        let ids = source.map { queue[$0].queueItemId }

        if destination == 0 {
            playback.moveInQueue(queueItemIds: ids, target: queue[0].queueItemId, after: false)
        } else {
            let target = queue[destination - 1].queueItemId
            let movedDown = source.allSatisfy { $0 < destination }
            playback.moveInQueue(queueItemIds: ids, target: target, after: movedDown)
        }
    }
}
