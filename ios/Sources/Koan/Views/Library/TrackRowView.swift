import SwiftUI

struct TrackRowView: View {
    let track: Track
    var showAlbum = true
    var addToQueue: () async -> Void = {}
    var toggleFavourite: () async -> Void = {}

    var body: some View {
        HStack(spacing: 8) {
            if let number = track.trackNumber {
                Text("\(number)")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(width: 28, alignment: .trailing)
                    .monospacedDigit()
            }

            VStack(alignment: .leading, spacing: 2) {
                Text(track.title)
                    .font(.body)
                    .lineLimit(1)

                HStack(spacing: 4) {
                    Text(track.artist)
                    if showAlbum {
                        Text("\u{00B7} \(track.album)")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }

            Spacer()

            if track.isFavourite == true {
                Image(systemName: "heart.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            Text(track.formattedDuration)
                .font(.callout)
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
        .contextMenu {
            Button {
                Task { await addToQueue() }
            } label: {
                Label("Add to Queue", systemImage: "text.append")
            }
            Divider()
            Button {
                Task { await toggleFavourite() }
            } label: {
                Label(
                    track.isFavourite == true ? "Remove from Favourites" : "Add to Favourites",
                    systemImage: track.isFavourite == true ? "heart.slash" : "heart"
                )
            }
        }
    }
}
