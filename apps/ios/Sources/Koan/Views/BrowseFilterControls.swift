import KoanFFI
import SwiftUI

/// The album, artist and track browsers' filters, behind one control that says how
/// many are on: a popover on the Mac, a sheet on iOS. The same filters as the
/// web UI's toolbar, answered by the same query.
struct BrowseFilterButton: View {
    @Environment(LibraryModel.self) private var library
    @State private var open = false

    var body: some View {
        let count = library.browseFilter.activeCount
        let symbol = Icon.filters
        Button { open = true } label: {
            // In a row above the listing, with room for the name.
            KoanLabel(count > 0 ? "Filters (\(count))" : "Filters", icon: symbol)
        }
        .help(count > 0 ? "Filters — \(count) on" : "Filters")
        .modifier(FilterSheet(open: $open))
    }
}

/// The filters on a television: the theme's panel, or the system's sheet.
/// Menu closes either.
private struct FilterSheet: ViewModifier {
    @Binding var open: Bool

    func body(content: Content) -> some View {
        if KoanTheme.isOn {
            content.televisionPanel(isPresented: $open, title: "Filter") { BrowseFilterForm() }
        } else {
            content.sheet(isPresented: $open) {
                NavigationStack {
                    BrowseFilterForm()
                        .navigationTitle(KoanTheme.label("Filter"))
                        .toolbar {
                            ToolbarItem(placement: .confirmationAction) { // theme: raw — a television's bar has no glass
                                Button("Done") { open = false }
                                    .toolbarButton()
                            }
                        }
                }
                .koanSheet()
                .presentationDetents([.medium, .large])
            }
        }
    }
}

struct BrowseFilterForm: View {
    @Environment(LibraryModel.self) private var library
    @FocusState private var year: YearBound?

    var body: some View {
        KoanForm { sections }
            .task { await library.loadBrowseChoices() }
            .decimalPadDone($year, order: [.from, .to])
    }

    @ViewBuilder private var sections: some View {
        Section { toggles }
        Section { pickers }
        Section { reset }
    }

    @ViewBuilder private var toggles: some View {
        @Bindable var library = library
        Toggle("Favourites", isOn: $library.browseFilter.favourites)
        Toggle("Recently Played", isOn: $library.browseFilter.recent)
        Toggle("Downloaded", isOn: $library.browseFilter.downloaded)
        // A track's codec says this already, and the track listing
        // filters by codec rather than by what its record is in.
        if library.section != .tracks {
            Toggle("Lossless", isOn: $library.browseFilter.lossless)
        }
    }

    @ViewBuilder private var pickers: some View {
        @Bindable var library = library
        KoanPicker(
            "Codec",
            selection: $library.browseFilter.codec,
            options: choices(offered(library.browseChoices?.codecs, current: library.browseFilter.codec)),
            keepsCase: true
        )
        KoanPicker(
            "Genre",
            selection: $library.browseFilter.genre,
            options: choices(offered(library.browseChoices?.genres, current: library.browseFilter.genre)),
            keepsCase: true
        )
        LabeledContent("Years") {
            HStack(spacing: 4) {
                YearField(prompt: "From", value: $library.browseFilter.yearFrom, focused: year == .from)
                    .focused($year, equals: .from)
                Text("–").foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                YearField(prompt: "To", value: $library.browseFilter.yearTo, focused: year == .to)
                    .focused($year, equals: .to)
            }
        }
    }

    private var reset: some View {
        Button("Reset") { library.browseFilter = .none }
            .koanButton(.standard)
            .disabled(library.browseFilter.activeCount == 0)
    }

    /// "Any", then the library's own values as they are written.
    private func choices(_ values: [String]) -> [(label: String, value: String?)] {
        [(KoanTheme.label("Any"), nil)] + values.map { ($0, $0) }
    }

    /// The choices, with the current one kept even when the library no longer
    /// has it, so a filter that is on can always be seen.
    private func offered(_ choices: [String]?, current: String?) -> [String] {
        let choices = choices ?? []
        guard let current, !choices.contains(current) else { return choices }
        return choices + [current]
    }
}

/// Which end of the year range has the keyboard.
private enum YearBound: Hashable {
    case from, to
}

/// A year, applied as it is typed rather than on Return: the phone's number
/// pad has none, and closing the sheet or popover with the field focused would
/// otherwise drop it. Only a whole year or an empty field is applied, so
/// typing 1990 is one query rather than four. A part-typed year left behind
/// goes back to the year applied, so the field never shows a filter that is
/// not on.
private struct YearField: View {
    let prompt: String
    @Binding var value: Int32?
    let focused: Bool
    @State private var text = ""

    var body: some View {
        TextField(prompt, text: $text, prompt: Text(KoanTheme.label(prompt)))
            .labelsHidden()
            .multilineTextAlignment(.center)
            // tvOS draws a rounded platter no style removes; the theme draws
            // its own box over the field.
            .koanField(text, prompt: KoanTheme.label(prompt))
            .frame(width: KoanTheme.metric(180, system: 60))
            .onAppear { text = value.map(String.init) ?? "" }
            .onChange(of: text) { _, typed in
                let digits = String(typed.filter(\.isASCII).filter(\.isNumber).prefix(4))
                if digits != typed { text = digits; return }
                if digits.isEmpty {
                    value = nil
                } else if digits.count == 4 {
                    value = Int32(digits)
                }
            }
            .onChange(of: focused) { _, now in
                if !now, !text.isEmpty, text.count < 4 { text = value.map(String.init) ?? "" }
            }
            // Reset, from outside the field.
            .onChange(of: value) { _, now in
                if now == nil, text.count == 4 { text = "" }
            }
    }
}

