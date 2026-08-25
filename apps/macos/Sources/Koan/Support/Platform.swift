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
