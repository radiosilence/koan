import KoanFFI
import SwiftUI

/// Saved queues. koan calls them snapshots because they capture playback
/// position too, not just a track list.
struct SnapshotsView: View {
    @Environment(LibraryModel.self) private var library
    @Environment(PlayerModel.self) private var player

    @State private var newName = ""
    @State private var creating = false

    var body: some View {
        VStack(spacing: 0) {
            if library.snapshots.isEmpty {
                EmptyState(
                    icon: "bookmark",
                    title: "No snapshots",
                    detail: "Save the current queue to come back to it later."
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(library.snapshots, id: \.name) { snapshot in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(snapshot.name)
                                    .font(.callout.weight(.medium))
                                Text("\(Format.count(Int64(snapshot.trackCount), "track")) · resumes at \(Format.duration(snapshot.positionMs))")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            Button("Restore") {
                                Task { try? await player.engine.restoreSnapshot(name: snapshot.name) }
                            }
                            Button {
                                library.deleteSnapshot(name: snapshot.name)
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(.secondary)
                        }
                        .padding(.vertical, 4)
                    }
                }
                .listStyle(.inset)
            }
        }
        .toolbar {
            ToolbarItem {
                Button {
                    creating = true
                } label: {
                    Label("Save Queue", systemImage: "plus")
                }
                .disabled(player.queue.isEmpty)
            }
        }
        .alert("Save Queue", isPresented: $creating) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Save") {
                guard !newName.isEmpty else { return }
                library.saveSnapshot(name: newName)
                newName = ""
            }
        }
    }
}
