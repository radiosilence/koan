import SwiftUI

/// Narrows what the current view is showing.
///
/// Deliberately separate from the sidebar's search field. That one is a
/// library-wide lookup that navigates you somewhere; this one only hides rows
/// in front of you, and conflating the two means you cannot say "of the albums
/// I am looking at, the ones with 'live' in the title" without being thrown
/// into a results page.
///
/// Clears itself when you leave the view — a filter you cannot see is a filter
/// you will not remember setting, and coming back to an apparently empty
/// library is a bug report waiting to happen.
struct FilterField: View {
    let placeholder: String

    @Environment(LibraryModel.self) private var library
    @FocusState private var focused: Bool

    var body: some View {
        @Bindable var library = library

        HStack(spacing: 5) {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.caption)
                .foregroundStyle(.tertiary)
            TextField(placeholder, text: $library.filter)
                .textFieldStyle(.plain)
                .font(.callout)
                .focused($focused)
                // Escape is what people press to abandon a filter.
                .onExitCommand { library.filter = "" }
            if !library.filter.isEmpty {
                Button {
                    library.filter = ""
                    focused = true
                } label: {
                    Image(systemName: "xmark.circle.fill")
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tertiary)
                .help("Clear filter")
            }
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
        .frame(width: 190)
    }
}
