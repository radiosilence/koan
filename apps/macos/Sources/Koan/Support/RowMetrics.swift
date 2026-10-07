import CoreGraphics
import SwiftUI

/// How tall a list's rows are, on every page. Rows holding the same things are
/// the same height wherever they appear, so moving between the queue, a
/// record and history does not change how dense the page is.
///
/// Heights are of the row's contents. A list adds its own padding above and
/// below — four points on the Mac, which `KoanTable` matches — so the rows on
/// screen are these plus that.
enum RowMetrics {
    #if os(macOS)
    /// A row of one line: an artist.
    static let line: CGFloat = 24
    /// A row of text: a track's title, and its credit under it when it has one.
    static let text: CGFloat = 34
    /// A row with a sleeve beside two lines of text.
    static let art: CGFloat = 40
    /// The sleeve in such a row.
    static let sleeve: CGFloat = 32
    /// What a list puts above and below each row. More in the theme, whose
    /// rows have no rules between them: the space is what tells them apart.
    /// Read from the theme each time, so a row measured before the theme is
    /// applied at launch does not keep the platform's spacing.
    static var padding: CGFloat { KoanTheme.isOn ? 7 : 4 }
    /// What a row with a sleeve adds above and below, so the cover clears the
    /// separators.
    static let artPadding: CGFloat = 4
    #else
    /// The least a row stands, before the list's own insets. Taller for a
    /// title on two lines.
    static let line: CGFloat = 44
    static let text: CGFloat = 34
    static let art: CGFloat = 42
    static let sleeve: CGFloat = 32
    static let artPadding: CGFloat = 4
    #endif
}

extension GridItem {
    /// Columns of record tiles, as many as fit. A television is read from
    /// across a room, so there they come about six across whatever is asked.
    static func tiles(minimum: CGFloat, maximum: CGFloat, spacing: CGFloat) -> [GridItem] {
        #if os(tvOS)
        [GridItem(.adaptive(minimum: 250, maximum: 300), spacing: 48)]
        #else
        [GridItem(.adaptive(minimum: minimum, maximum: maximum), spacing: spacing)]
        #endif
    }
}
