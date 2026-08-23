import SwiftUI

/// The heart, wherever something can be favourited.
///
/// One view so a track row, a queue row, an album and an artist all behave the
/// same: filled and red when on, and otherwise only visible on hover so a list
/// of forty tracks is not a column of grey hearts.
struct FavouriteButton: View {
    let isOn: Bool
    /// Whether to show it while it is off — hover, usually.
    var showing: Bool = true
    var size: Font = .body
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: isOn ? "heart.fill" : "heart")
                .font(size)
                .foregroundStyle(isOn ? AnyShapeStyle(.red) : AnyShapeStyle(.tertiary))
                .contentTransition(.symbolEffect(.replace))
        }
        .buttonStyle(.plain)
        .opacity(isOn || showing ? 1 : 0)
        .help(isOn ? "Remove favourite" : "Favourite")
        .accessibilityLabel(isOn ? "Remove favourite" : "Favourite")
    }
}
