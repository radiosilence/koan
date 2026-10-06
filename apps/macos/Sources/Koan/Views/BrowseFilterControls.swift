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
        let symbol = count > 0
            ? "line.3.horizontal.decrease.circle.fill"
            : "line.3.horizontal.decrease.circle"
        Button { open = true } label: {
            #if os(tvOS)
            // In a row above the listing, with room for the name.
            Label(count > 0 ? "Filters (\(count))" : "Filters", systemImage: symbol)
            #else
            HStack(spacing: 3) {
                Image(systemName: symbol)
                if count > 0 {
                    Text("\(count)").monospacedDigit()
                }
            }
            .accessibilityLabel(count > 0 ? "Filters, \(count) on" : "Filters")
            #endif
        }
        .help(count > 0 ? "Filters — \(count) on" : "Filters")
        #if os(macOS)
        .tint(.primary)
        .popover(isPresented: $open, arrowEdge: .bottom) {
            BrowseFilterForm()
                .frame(width: 300)
                .koanPopover()
        }
        #else
        .sheet(isPresented: $open) {
            NavigationStack {
                BrowseFilterForm()
                    .navigationTitle(KoanTheme.label("Filter"))
                    #if !os(tvOS)
                    .navigationBarTitleDisplayMode(.inline)
                    #endif
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { open = false }
                                .toolbarButton()
                        }
                    }
            }
            .presentationDetents([.medium, .large])
        }
        #endif
    }
}

private struct BrowseFilterForm: View {
    @Environment(LibraryModel.self) private var library

    var body: some View {
        @Bindable var library = library
        Form {
            Section {
                Toggle("Favourites", isOn: $library.browseFilter.favourites)
                Toggle("Recently Played", isOn: $library.browseFilter.recent)
                Toggle("Downloaded", isOn: $library.browseFilter.downloaded)
                // A track's codec says this already, and the track listing
                // filters by codec rather than by what its record is in.
                if library.section != .tracks {
                    Toggle("Lossless", isOn: $library.browseFilter.lossless)
                }
            }
            Section {
                Picker("Codec", selection: $library.browseFilter.codec) {
                    Text("Any").tag(String?.none)
                    ForEach(offered(library.browseChoices?.codecs, current: library.browseFilter.codec), id: \.self) {
                        Text($0).tag(String?.some($0))
                    }
                }
                Picker("Genre", selection: $library.browseFilter.genre) {
                    Text("Any").tag(String?.none)
                    ForEach(offered(library.browseChoices?.genres, current: library.browseFilter.genre), id: \.self) {
                        Text($0).tag(String?.some($0))
                    }
                }
                LabeledContent("Years") {
                    HStack(spacing: 4) {
                        YearField(prompt: "From", value: $library.browseFilter.yearFrom)
                        Text("–").foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                        YearField(prompt: "To", value: $library.browseFilter.yearTo)
                    }
                }
            }
            Section {
                Button("Reset") { library.browseFilter = .none }
                    .disabled(library.browseFilter.activeCount == 0)
            }
        }
        #if os(macOS)
        .formStyle(.grouped)
        #endif
        .task { await library.loadBrowseChoices() }
    }

    /// The choices, with the current one kept even when the library no longer
    /// has it, so a filter that is on can always be seen.
    private func offered(_ choices: [String]?, current: String?) -> [String] {
        let choices = choices ?? []
        guard let current, !choices.contains(current) else { return choices }
        return choices + [current]
    }
}

/// A year, applied as it is typed rather than on Return: the phone's number
/// pad has none, and closing the sheet or popover with the field focused would
/// otherwise drop it. Only a whole year or an empty field is applied, so
/// typing 1990 is one query rather than four.
private struct YearField: View {
    let prompt: String
    @Binding var value: Int32?
    @State private var text = ""

    var body: some View {
        TextField(prompt, text: $text, prompt: Text(prompt))
            .labelsHidden()
            .multilineTextAlignment(.center)
            .frame(width: 60)
            #if os(iOS)
            .keyboardType(.numberPad)
            #endif
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
            // Reset, from outside the field.
            .onChange(of: value) { _, now in
                if now == nil, text.count == 4 { text = "" }
            }
    }
}
