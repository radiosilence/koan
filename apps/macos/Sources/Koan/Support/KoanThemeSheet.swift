#if os(macOS)
import SwiftUI

/// Every component of the kōan theme on one page, for the evidence renderer:
/// what a change to the layer is checked against before any screen is.
struct KoanThemeSheet: View {
    var accent: KoanAccent = .mint
    @State private var on = true
    @State private var off = false
    @State private var segment = 0
    @State private var level = 0.38

    var body: some View {
        VStack(alignment: .leading, spacing: KoanTheme.Space.l) {
            Text("kōan").koanText(.display, .strong)
            Text("albums").koanText(.title, .strong)
            Text("Low Tide Arcade").koanText(.titleSmall, .strong)
            Text("Body text, in ink.").koanText(.body)
            Text("Secondary text, muted.").koanText(.meta, .muted)
            Text("The record's accent, as text.").koanText(.body, .accent)
            Text("Something went wrong.").koanText(.fine, .bad)

            KoanSectionHeader("Buttons")
            HStack(spacing: KoanTheme.Space.m) {
                Button { } label: { KoanLabel("Play", icon: Icon.play) }.koanButton(.prominent)
                Button { } label: { KoanLabel("Shuffle", icon: Icon.shuffle) }.koanButton(.standard)
                Button("Clear") { }.koanButton(.text)
                Button { } label: { KoanLabel("Sleep", icon: "moon", style: .compact) }.koanButton(.icon)
                Button { } label: { Image(systemName: "pause.fill") }.koanButton(.iconOutlined)
                Button("Disabled") { }.koanButton(.standard).disabled(true)
            }

            KoanSectionHeader("Toggles")
            Toggle("Show icons", isOn: $on).koanToggle()
            Toggle("Gapless", isOn: $off).koanToggle()

            KoanSectionHeader("Segmented")
            KoanSegmentedPicker(
                options: [("All", 0), ("Favourites", 1), ("Lossless", 2)],
                selection: $segment,
                title: "Filter"
            )

            KoanSectionHeader("Slider")
            Slider(value: $level)

            KoanSectionHeader("Rows")
            VStack(spacing: 0) {
                ForEach(["Shoreline Pinball", "Coin Return", "Neon Breakwater"], id: \.self) { title in
                    HStack {
                        Text(title).koanText(.body, title == "Neon Breakwater" ? .accent : .ink)
                        Spacer()
                        Text("3:41").koanText(.meta, .muted)
                    }
                    .padding(.vertical, 12)
                    .koanRow(selected: title == "Coin Return")
                }
            }

            KoanSectionHeader("Form")
            KoanForm {
                Section {
                    Toggle("Gapless", isOn: $off)
                    LabeledContent("Name") {
                        TextField("Name", text: .constant("Sunday morning")).koanField()
                    }
                } header: {
                    KoanSectionHeader("Playback")
                } footer: {
                    Text("A footer that explains the section, long enough to wrap onto a second line.")
                        .koanText(.fine, .muted)
                }
            }
            .frame(height: 220)

            KoanSectionHeader("Surface")
            Text("search").koanText(.body, .muted)
                .padding(KoanTheme.Space.m)
                .frame(maxWidth: .infinity, alignment: .leading)
                .koanSurface(.surface)
        }
        .padding(KoanTheme.Space.xxl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .koanSurface()
        .tint(accent.color)
        .environment(\.koanAccent, accent)
    }
}
#endif
