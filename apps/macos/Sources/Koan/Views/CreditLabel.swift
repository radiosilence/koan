#if os(macOS)
import AppKit
import KoanFFI

/// An artist credit drawn in one AppKit label, as the Mac's tiles and rows
/// draw it: which credited artist the pointer is over, and the credit with
/// that one artist marked as a link. `CreditLinks` is the SwiftUI side.
enum CreditLabel {
    static func text(_ parts: [CreditPart]) -> String {
        parts.map(\.text).joined()
    }

    /// The part `x` points into, measured from the leading edge of a label
    /// `width` wide, when that part is a credited artist's name. Nothing under
    /// the ellipsis of a credit cut short, whose names cannot be seen.
    static func artist(at x: CGFloat, in parts: [CreditPart], font: NSFont, width: CGFloat) -> Int? {
        // A label insets its text by two points a side.
        let measure = { (text: String) in (text as NSString).size(withAttributes: [.font: font]).width }
        if measure(text(parts)) + 4 > width, x >= width - 2 - measure("…") { return nil }
        var right: CGFloat = 2
        for (i, part) in parts.enumerated() {
            right += measure(part.text)
            if x < right { return part.artistId == nil ? nil : i }
        }
        return nil
    }

    /// The credit in `colour`, the part at `linked` in `linkColour` and
    /// underlined.
    static func attributed(
        _ parts: [CreditPart], font: NSFont, colour: NSColor, linked: Int?, linkColour: NSColor
    ) -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail
        let out = NSMutableAttributedString()
        for (i, part) in parts.enumerated() {
            var attributes: [NSAttributedString.Key: Any] = [
                .font: font,
                .foregroundColor: i == linked ? linkColour : colour,
                .paragraphStyle: paragraph,
            ]
            if i == linked { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            out.append(NSAttributedString(string: part.text, attributes: attributes))
        }
        return out
    }
}
#endif
