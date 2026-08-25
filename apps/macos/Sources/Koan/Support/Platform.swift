import SwiftUI
#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif

/// The handful of places where AppKit and UIKit disagree about a type koan
/// actually uses.
///
/// Everything else in the app is SwiftUI and crosses on its own. This exists so
/// the art pipeline — which is `CGImageSource` end to end and only meets a
/// platform image at the very last step — doesn't have to be written twice.
#if canImport(AppKit)
typealias PlatformImage = NSImage
#else
typealias PlatformImage = UIImage
#endif

extension PlatformImage {
    /// Wrap a decoded bitmap without redrawing it.
    ///
    /// The size is in pixels, which is what the decode produced: AppKit wants it
    /// stated, UIKit infers it from the `CGImage` and ignores what it is told.
    static func decoded(_ bitmap: CGImage, pixelSize: CGSize) -> PlatformImage {
        #if canImport(AppKit)
        NSImage(cgImage: bitmap, size: pixelSize)
        #else
        UIImage(cgImage: bitmap)
        #endif
    }

    /// The bitmap behind the image, for sampling rather than drawing.
    var bitmap: CGImage? {
        #if canImport(AppKit)
        var rect = NSRect(origin: .zero, size: size)
        return cgImage(forProposedRect: &rect, context: nil, hints: nil)
        #else
        return cgImage
        #endif
    }
}

extension Image {
    init(platform: PlatformImage) {
        #if canImport(AppKit)
        self.init(nsImage: platform)
        #else
        self.init(uiImage: platform)
        #endif
    }
}

extension View {
    /// A field holding a machine value — a URL, an account name, a format
    /// string — rather than prose.
    ///
    /// iOS assumes prose: it capitalises the first letter and autocorrects as
    /// you go, which turns `https://music.blit.cc` into `HTTPS://music.blit.cc`
    /// and quietly ruins a password. A Mac keyboard does none of that, so this
    /// is a no-op there.
    func verbatimEntry(_ contentType: VerbatimContent = .plain) -> some View {
        #if os(macOS)
        self
        #else
        autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .keyboardType(contentType.keyboard)
        #endif
    }
}

/// What kind of machine value, for the keyboard iOS should offer.
enum VerbatimContent {
    case plain
    case url

    #if !os(macOS)
    var keyboard: UIKeyboardType {
        switch self {
        case .plain: .asciiCapable
        case .url: .URL
        }
    }
    #endif
}

extension View {
    /// Several buttons sharing one row of a `Form` or `List`.
    ///
    /// iOS treats such a row as a single tap target unless each button opts out
    /// of the row's own behaviour, and resolves a tap to one of them regardless
    /// of where it landed — which is how "Sync Now" signed you out. `.borderless`
    /// is what gives them their own hit testing.
    ///
    /// macOS gets the bordered buttons it already had: a row there is not a
    /// control, so there is nothing to opt out of.
    func rowButtons() -> some View {
        #if os(macOS)
        self
        #else
        buttonStyle(.borderless)
        #endif
    }
}
