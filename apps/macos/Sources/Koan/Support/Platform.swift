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

extension View {
    /// A row's primary action: open the record, play the track.
    ///
    /// macOS puts this on the `List` itself, through
    /// `contextMenu(forSelectionType:menu:primaryAction:)` — wired into the
    /// selection machinery rather than the gesture system, which is what keeps
    /// it from stealing the first click. That mechanism means *double*-click,
    /// and it needs a selection to act on.
    ///
    /// A phone has neither. Touch has no double-click, and a `List` selection
    /// on iOS only exists in edit mode — so every browse list in the app was
    /// inert: tapping an artist, a track or a history row did nothing at all.
    /// Here the row takes the tap itself.
    func primaryTap(_ action: @escaping () -> Void) -> some View {
        #if os(macOS)
        self
        #else
        contentShape(Rectangle()).onTapGesture(perform: action)
        #endif
    }
}

/// Labels keep their words while there is room, and lose them when there is not.
///
/// A row of `Label` buttons is fine beside a sleeve in a window and impossible
/// on a phone, where each one wraps a character at a time. The symbols are the
/// same ones the context menus use, so nothing is lost but the words.
private struct IconOnlyWhenTight: ViewModifier {
    @Environment(\.horizontalSizeClass) private var width

    func body(content: Content) -> some View {
        if width == .compact {
            content.labelStyle(.iconOnly)
        } else {
            content
        }
    }
}

extension View {
    func iconOnlyWhenTight() -> some View { modifier(IconOnlyWhenTight()) }
}

extension Notification.Name {
    /// The app giving up the foreground — on iOS the last dependable moment
    /// before it is suspended and perhaps killed without another word.
    static var appResignsActive: Notification.Name {
        #if canImport(AppKit)
        NSApplication.didResignActiveNotification
        #else
        UIApplication.willResignActiveNotification
        #endif
    }
}

#if canImport(AppKit)
typealias PlatformColor = NSColor
#else
typealias PlatformColor = UIColor
#endif

/// A view whose drawing is its own sublayers, which is how koan keeps motion in
/// the render server rather than on the main thread.
///
/// AppKit and UIKit agree on the layers and disagree on everything around them:
/// whether there is a layer at all, which method is the layout pass, and how a
/// change of light or dark is announced. Subclasses override `layoutLayers` and
/// `appearanceChanged` and never meet the difference.
class LayerView: PlatformView {
    /// Never nil: an AppKit view is made layer-backed here, a UIKit one always is.
    var hostLayer: CALayer {
        #if canImport(AppKit)
        layer!
        #else
        layer
        #endif
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        #if canImport(AppKit)
        wantsLayer = true
        #else
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: LayerView, _) in
            view.appearanceChanged()
        }
        #endif
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    /// The size may have changed.
    func layoutLayers() {}

    /// Light and dark swapped; anything holding a resolved `CGColor` repaints.
    func appearanceChanged() {}

    /// A dynamic colour pinned to this view's current appearance.
    func resolved(_ colour: PlatformColor) -> CGColor {
        #if canImport(AppKit)
        var resolved = colour.cgColor
        effectiveAppearance.performAsCurrentDrawingAppearance { resolved = colour.cgColor }
        return resolved
        #else
        colour.resolvedColor(with: traitCollection).cgColor
        #endif
    }

    #if canImport(AppKit)
    override func layout() {
        super.layout()
        layoutLayers()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        appearanceChanged()
    }
    #else
    override func layoutSubviews() {
        super.layoutSubviews()
        layoutLayers()
    }
    #endif
}

#if canImport(AppKit)
typealias PlatformView = NSView
#else
typealias PlatformView = UIView
#endif

/// `NSViewRepresentable` and `UIViewRepresentable` under one set of names.
///
/// Conformers name `PlatformViewType` outright. Inferring it through the
/// platform's own associated type works for some and not others, depending on
/// the order the compiler happens to meet them in.
#if canImport(AppKit)
protocol PlatformViewRepresentable: NSViewRepresentable where NSViewType == PlatformViewType {
    associatedtype PlatformViewType: NSView
    func makeView(context: Context) -> PlatformViewType
    func updateView(_ view: PlatformViewType, context: Context)
    static func dismantleView(_ view: PlatformViewType, coordinator: Coordinator)
}

extension PlatformViewRepresentable {
    func makeNSView(context: Context) -> PlatformViewType { makeView(context: context) }
    func updateNSView(_ view: PlatformViewType, context: Context) {
        updateView(view, context: context)
    }
    static func dismantleNSView(_ view: PlatformViewType, coordinator: Coordinator) {
        dismantleView(view, coordinator: coordinator)
    }
    static func dismantleView(_ view: PlatformViewType, coordinator: Coordinator) {}
}
#else
protocol PlatformViewRepresentable: UIViewRepresentable where UIViewType == PlatformViewType {
    associatedtype PlatformViewType: UIView
    func makeView(context: Context) -> PlatformViewType
    func updateView(_ view: PlatformViewType, context: Context)
    static func dismantleView(_ view: PlatformViewType, coordinator: Coordinator)
}

extension PlatformViewRepresentable {
    func makeUIView(context: Context) -> PlatformViewType { makeView(context: context) }
    func updateUIView(_ view: PlatformViewType, context: Context) {
        updateView(view, context: context)
    }
    static func dismantleUIView(_ view: PlatformViewType, coordinator: Coordinator) {
        dismantleView(view, coordinator: coordinator)
    }
    static func dismantleView(_ view: PlatformViewType, coordinator: Coordinator) {}
}
#endif

#if canImport(AppKit)
extension NSColor {
    /// UIKit's name for it, so shared code can say one thing.
    static var label: NSColor { labelColor }
}
#endif

extension View {
    /// A button that reads as a link. iOS has no link style; a borderless
    /// button in the tint is what it uses for the same job.
    func linkButton() -> some View {
        #if os(macOS)
        buttonStyle(.link)
        #else
        buttonStyle(.borderless)
        #endif
    }
}
