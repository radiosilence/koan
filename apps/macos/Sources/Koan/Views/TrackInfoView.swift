import KoanFFI
import SwiftUI

/// Everything the library knows about a track, as each of its sources says
/// it: the file's tags, the server's entry, the ReplayGain in the file. Small
/// and dense, for reading a value off or copying it.
struct TrackInfoView: View {
    let info: TrackInfo

    var body: some View {
        VStack(alignment: .leading, spacing: KoanTheme.Space.l) {
            Text(info.track.title)
                .koanText(.body, .strong)
            ForEach(Array(info.sources.enumerated()), id: \.offset) { _, source in
                group(source.kind == "server" ? "Server" : "File", source.fields)
            }
            if !info.replayGain.isEmpty {
                group("ReplayGain", info.replayGain)
            }
            group("Library", ids)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.top, 12)
        .padding(.bottom, KoanTheme.Space.l)
        .selectableText()
    }

    private var ids: [InfoField] {
        [InfoField(name: "Track id", value: String(info.track.id))]
            + (info.uid.map { [InfoField(name: "Uid", value: $0)] } ?? [])
    }

    private func group(_ title: String, _ fields: [InfoField]) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            KoanSectionHeader(title)
            ForEach(fields, id: \.name) { field in
                HStack(alignment: .firstTextBaseline, spacing: KoanTheme.Space.s) {
                    Text(field.name)
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        .frame(width: 128, alignment: .leading)
                    Text(field.value)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(.role(.fine, system: .caption))
                .accessibilityElement(children: .combine)
            }
        }
    }
}
