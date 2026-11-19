import KoanFFI
import SwiftUI

/// A profile's filters as a table, its parametric bands editable in place:
/// number, type, frequency, gain and Q. An edit plays at once. A graphic
/// curve opens a page of its points; delays and mixes are shown as they are.
struct BandTable: View {
    let dsp: DspModel
    let profile: String
    let bands: [DspBand]
    /// A correction, which plays as made: its bands shown, not edited.
    var readOnly = false
    /// For an EQ with bands for each channel, the channel shown: its bands
    /// alone, and a band added goes on it.
    var channel: UInt16?
    /// The figure being typed, kept by the page: it shortens the graph and
    /// steps between fields from the keyboard.
    var focus: FocusState<BandField?>.Binding

    /// The band types that can be chosen, by the name the config uses: the
    /// short form the row shows and the name the menu gives.
    static let kinds: [(id: String, short: String, name: String)] = [
        ("peaking", "PK", "Peak"),
        ("low_shelf", "LS", "Low shelf"),
        ("high_shelf", "HS", "High shelf"),
        ("low_pass", "LP", "Low pass"),
        ("high_pass", "HP", "High pass"),
        ("notch", "NO", "Notch"),
        ("band_pass", "BP", "Band pass"),
        ("all_pass", "AP", "All pass"),
    ]

    /// The table is read across rows of figures, so it is set small and tight.
    static let rowInsets = EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 16)

    static func editable(_ kind: String) -> Bool {
        kinds.contains { $0.id == kind }
    }

    /// The bands the graph draws a handle for: the ones with a gain to drag,
    /// on the channel the graph draws.
    static func handles(_ bands: [DspBand], channel: UInt16 = 0) -> [EqGraph.Handle] {
        bands.enumerated().compactMap { i, b in
            ["peaking", "low_shelf", "high_shelf"].contains(b.kind) && (b.channels.isEmpty || b.channels.contains(channel))
                ? EqGraph.Handle(index: i, hz: b.freq, db: b.gainDb, q: b.q)
                : nil
        }
    }

    /// The rows shown, each with its index among all the filters.
    private var shown: [(index: Int, band: DspBand)] {
        Self.shown(bands, channel: channel)
    }

    private static func shown(_ bands: [DspBand], channel: UInt16?) -> [(index: Int, band: DspBand)] {
        bands.enumerated()
            .filter { _, b in channel.map { b.channels.isEmpty || b.channels.contains($0) } ?? true }
            .map { (index: $0.offset, band: $0.element) }
    }

    /// Every figure that can be typed into, in the order the rows show them.
    static func fields(_ bands: [DspBand], channel: UInt16?, readOnly: Bool) -> [BandField] {
        guard !readOnly else { return [] }
        return shown(bands, channel: channel)
            .filter { editable($0.band.kind) }
            .flatMap { row in BandField.Part.allCases.map { BandField(index: row.index, part: $0) } }
    }

    var body: some View {
        Section {
            if !bands.isEmpty {
                HStack(spacing: 8) {
                    Text("#").frame(width: 22, alignment: .leading)
                    Text("Type").frame(maxWidth: .infinity, alignment: .leading)
                    Text("Hz").frame(width: 72, alignment: .trailing)
                    Text("dB").frame(width: 56, alignment: .trailing)
                    Text("Q").frame(width: 50, alignment: .trailing)
                }
                .koanText(.fine, .muted)
                .listRowInsets(Self.rowInsets)
            }
            ForEach(Array(shown.enumerated()), id: \.element.index) { position, row in
                let (index, band) = row
                Group {
                    if Self.editable(band.kind), !readOnly {
                        BandEditor(dsp: dsp, profile: profile, index: index, number: position + 1, band: band, focus: focus)
                            .id(BandField.row(index))
                    } else if band.kind == "graphic", !readOnly {
                        BandRow(band: band)
                    } else {
                        BandRow(band: band)
                    }
                }
                .font(.role(.meta, system: .body))
                .listRowInsets(Self.rowInsets)
            }
        } header: {
            KoanSectionHeader("Filters")
        } footer: {
            Text(readOnly
                 ? "A correction plays as made. To change the sound, add a tuning on top; to edit these, make it a tuning under What it's for."
                 : "Edits play at once. Frequency, gain and Q are held to 10 Hz–22 kHz, ±30 dB and 0.1–20.")
                .koanText(.fine, .muted)
        }
    }
}

/// A figure of a band's row: the band's index among all the filters, and
/// which of its three.
struct BandField: Hashable {
    let index: Int
    let part: Part

    enum Part: CaseIterable { case freq, gain, q }

    /// What a band's row is known by, to scroll it into view.
    static func row(_ index: Int) -> String { "band-\(index)" }
}

/// One band's row, each field committed when it is left or returned.
private struct BandEditor: View {
    let dsp: DspModel
    let profile: String
    let index: Int
    /// Its place in the list shown.
    let number: Int
    let band: DspBand
    var focus: FocusState<BandField?>.Binding

    @State private var kind = ""
    @State private var freq = 0.0
    @State private var gain = 0.0
    @State private var q = 0.0

    var body: some View {
        HStack(spacing: 8) {
            Text("\(number)")
                .koanText(.meta, .muted)
                .monospacedDigit()
                .frame(width: 22, alignment: .leading)
            // The short form in the row, the names in the menu.
            Menu {
                Picker("Type", selection: Binding(get: { kind }, set: { kind = $0; commit() })) { // theme: raw — a menu's items
                    ForEach(BandTable.kinds, id: \.id) { Text($0.name).tag($0.id) }
                }
            } label: {
                Text(BandTable.kinds.first { $0.id == kind }?.short ?? kind)
                    .textCase(nil)
            }
            // Not `koanControl`, whose `control` type would outweigh the row's.
            .koanMenuButton(.text)
            .tint(KoanTheme.style(.ink, system: .tint))
            .accessibilityLabel(BandTable.kinds.first { $0.id == kind }?.name ?? kind)
            .frame(maxWidth: .infinity, alignment: .leading)
            NumberField(value: $freq, focus: focus, name: BandField(index: index, part: .freq), width: 72, digits: 0, label: "Band \(number) frequency", commit: commit)
            NumberField(value: $gain, focus: focus, name: BandField(index: index, part: .gain), width: 56, digits: 1, label: "Band \(number) gain", commit: commit)
            NumberField(value: $q, focus: focus, name: BandField(index: index, part: .q), width: 50, digits: 2, label: "Band \(number) Q", commit: commit)
        }
        .onChange(of: focus.wrappedValue) { was, _ in if was?.index == index { commit() } }
        .onAppear(perform: read)
        .onChange(of: band.freq) { _, _ in read() }
        .onChange(of: band.gainDb) { _, _ in read() }
        .onChange(of: band.q) { _, _ in read() }
        .onChange(of: band.kind) { _, _ in read() }
    }

    private func read() {
        (kind, freq, gain, q) = (band.kind, band.freq, band.gainDb, band.q)
    }

    private func commit() {
        guard kind != band.kind || freq != band.freq || gain != band.gainDb || q != band.q else { return }
        dsp.setBand(profile, index, kind: kind, freq: freq, gain: gain, q: q)
    }
}

/// A figure in a row of the table, committed when it is left or returned,
/// with a rule beneath it so it reads as a field to tap.
private struct NumberField<Field: Hashable>: View {
    @Binding var value: Double
    var focus: FocusState<Field?>.Binding
    let name: Field
    let width: CGFloat
    let digits: Int
    /// What VoiceOver reads it as: the field has no title of its own.
    let label: String
    let commit: () -> Void

    var body: some View {
        TextField("", value: $value, format: .number.precision(.fractionLength(0 ... digits)))
            .focused(focus, equals: name)
            .accessibilityLabel(label)
            .multilineTextAlignment(.trailing)
            .monospacedDigit()
            .frame(width: width)
            .onSubmit(commit)
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(KoanTheme.style(.rule, system: Color.secondary.opacity(0.4))) // theme: raw — the system look's own
                    .frame(height: KoanTheme.hairline)
                    .offset(y: 2)
            }
    }
}

extension View {
    /// The decimal pad has no return key: above it, the field before and
    /// after in `order` when there is more than one, and `label`, which
    /// commits the figure and puts the keyboard away. Declared once for the
    /// page, never per row: a toolbar in a list's row is laid out as the
    /// page's bottom bar.
    func decimalPadDone<Field: Hashable>(_ focus: FocusState<Field?>.Binding, order: [Field], label: LocalizedStringKey = "Done") -> some View {
        self
    }

}

