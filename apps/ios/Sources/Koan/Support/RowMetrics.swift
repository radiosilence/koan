import CoreGraphics
import SwiftUI

/// How tall a list's rows are, on every page. Rows holding the same things are
/// the same height wherever they appear, so moving between the queue, a
/// record and history does not change how dense the page is.
///
/// Heights are of the row's contents. A list adds its own padding above and
/// below, so the rows on screen are these plus that.
enum RowMetrics {
    /// The least a row stands, before the list's own insets. Taller for a
    /// title on two lines.
    static let line: CGFloat = 44
    static let text: CGFloat = 34
    static let art: CGFloat = 42
    static let sleeve: CGFloat = 32
    static let artPadding: CGFloat = 4

    /// A phone's rows: on the page's 16pt edge, with little above and below.
    /// The list's own insets leave a tracklist a few titles to a screen.
    /// Declared for every platform: the views naming it build on the Mac too.
    static let compactInsets = EdgeInsets(top: 4, leading: 16, bottom: 4, trailing: 16)
}

extension GridItem {
    /// Columns of record tiles, as many as fit. A television is read from
    /// across a room, so there they come about six across whatever is asked.
    static func tiles(minimum: CGFloat, maximum: CGFloat, spacing: CGFloat) -> [GridItem] {
        [GridItem(.adaptive(minimum: 250, maximum: 300), spacing: 48)]
    }
}
