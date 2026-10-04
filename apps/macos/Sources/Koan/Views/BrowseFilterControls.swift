import KoanFFI
import SwiftUI

/// The album and artist browsers' filters, behind one control that says how
/// many are on: a popover on the Mac, a sheet on iOS. The same filters as the
/// web UI's toolbar, answered by the same query.
struct BrowseFilterButton: View {
    @Environment(LibraryModel.self) private var library
    @State private var open = false

    var body: some View {
        let count = library.browseFilter.activeCount
        Button { open = true } label: {
            HStack(spacing: 3) {
                Image(systemName: count > 0
                    ? "line.3.horizontal.decrease.circle.fill"
                    : "line.3.horizontal.decrease.circle")
                if count > 0 {
                    Text("\(count)").monospacedDigit()
                }
            }
            .accessibilityLabel(count > 0 ? "Filters, \(count) on" : "Filters")
        }
        .help(count > 0 ? "Filters — \(count) on" : "Filters")
        #if os(macOS)
        .tint(.primary)
        .popover(isPresented: $open, arrowEdge: .bottom) {
            BrowseFilterForm()
                .frame(width: 300)
        }
        #else
        .sheet(isPresented: $open) {
            NavigationStack {
                BrowseFilterForm()
                    .navigationTitle("Filter")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .confirmationAction) {
                            Button("Done") { open = false }
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
                Toggle("Lossless", isOn: $library.browseFilter.lossless)
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
                        year("From", $library.browseFilter.yearFrom)
                        Text("–").foregroundStyle(.secondary)
                        year("To", $library.browseFilter.yearTo)
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

    private func year(_ prompt: String, _ value: Binding<Int32?>) -> some View {
        TextField(prompt, value: value, format: .number.grouping(.never), prompt: Text(prompt))
            .labelsHidden()
            .multilineTextAlignment(.center)
            .frame(width: 60)
            #if os(iOS)
            .keyboardType(.numberPad)
            #endif
    }
}
