import SwiftUI
import UniformTypeIdentifiers
#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif

/// The handful of places where AppKit and UIKit disagree about a type koan
/// uses.
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
    /// of where it landed, so "Sync Now" can sign you out. `.borderless` is
    /// what gives them their own hit testing.
    ///
    /// macOS keeps its bordered buttons: a row there is not a control, so there
    /// is nothing to opt out of.
    /// A television's borderless button is bare text until focused, which in a
    /// settings row reads as the row's value; there they keep their platters.
    func rowButtons() -> some View {
        #if os(macOS)
        self
        #elseif os(tvOS)
        buttonStyle(TelevisionButton())
        #else
        buttonStyle(.borderless)
        #endif
    }
}

extension View {
    /// A control drawn as its glyph alone on the Mac and the phone. A
    /// television gives it the round platter its neighbours have: a bare glyph
    /// shows no focus, and cannot be found from across the room.
    func controlButton() -> some View {
        #if os(tvOS)
        buttonStyle(TelevisionButton())
        #else
        buttonStyle(.plain)
        #endif
    }

    /// A button or menu in a toolbar. On tvOS it takes the system's toolbar
    /// style back from the shell's `TelevisionButton`, which would draw it as a
    /// capsule with its symbol at text size, and shows the symbol alone: a
    /// television's toolbar truncates a title to a letter or two.
    func toolbarButton() -> some View {
        #if os(tvOS)
        buttonStyle(.automatic).labelStyle(.iconOnly)
        #else
        self
        #endif
    }

    /// A `NavigationLink` in a list. On tvOS it is drawn as a full-width row:
    /// the shell's button style would otherwise make it a capsule the size of
    /// its label. Elsewhere it is a row on the wash.
    func listLink() -> some View {
        #if os(tvOS)
        buttonStyle(TelevisionRow(resting: 0.08))
        #else
        washedRow()
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
    /// on iOS only exists in edit mode, so here the row takes the tap itself.
    ///
    /// A television has no touch either: only what can take focus can be
    /// clicked, so there the row is a button.
    @ViewBuilder
    func primaryTap(_ action: @escaping () -> Void) -> some View {
        #if os(macOS)
        self
        #elseif os(tvOS)
        Button(action: action) { contentShape(Rectangle()) }
            .buttonStyle(TelevisionRow())
        #else
        contentShape(Rectangle()).onTapGesture(perform: action)
        #endif
    }
}

extension View {
    /// `contextMenu(forSelectionType:menu:primaryAction:)`, which tvOS does not
    /// have: a list there has no selection to act on, and its rows take their
    /// primary action through `primaryTap`.
    func selectionMenu<I: Hashable, M: View>(
        for type: I.Type,
        @ViewBuilder menu: @escaping (Set<I>) -> M,
        primaryAction: ((Set<I>) -> Void)? = nil
    ) -> some View {
        #if os(tvOS)
        self
        #else
        contextMenu(forSelectionType: type, menu: menu, primaryAction: primaryAction)
        #endif
    }
}

extension View {
    /// `primaryTap`, with the row's menu. A list hands rows their menus through
    /// its selection on the Mac and the phone; a television's list has none,
    /// so there the menu goes on the row's own button, where a long press of
    /// the remote finds it.
    @ViewBuilder
    func primaryTap<Menu: View>(
        _ action: @escaping () -> Void,
        @ViewBuilder menu: @escaping () -> Menu
    ) -> some View {
        #if os(tvOS)
        if KoanTheme.isOn {
            modifier(TelevisionMenuRow(action: action, menu: menu))
        } else {
            Button(action: action) { contentShape(Rectangle()) }
                .buttonStyle(TelevisionRow())
                .contextMenu { menu() }
        }
        #else
        primaryTap(action)
        #endif
    }
}

#if os(tvOS)
/// A row whose menu, in the theme, is a sheet of the theme's rows rather than
/// the system's popover of grey pills. A long press of the remote opens it,
/// as it does the system's. Focusable rather than a button: a button takes
/// the press for itself, and a long one never reaches the gesture.
private struct TelevisionMenuRow<Menu: View>: ViewModifier {
    let action: () -> Void
    @ViewBuilder let menu: () -> Menu
    @State private var open = false
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .frame(maxWidth: .infinity, alignment: .leading)
            .foregroundStyle(Color.koanInk)
            .padding(.horizontal, 20)
            .padding(.vertical, 6)
            .koanFocusRing(focused, gap: 0)
            .focusable()
            .focused($focused)
            .onLongPressGesture(minimumDuration: 0.5) { open = true }
            .onTapGesture(perform: action)
            .accessibilityAddTraits(.isButton)
            .accessibilityAction(named: "Menu") { open = true }
            .televisionPanel(isPresented: $open) {
                menu().buttonStyle(MenuItem(close: { open = false }))
            }
    }

    /// An item of the menu: runs, then closes the sheet, as a menu does.
    private struct MenuItem: PrimitiveButtonStyle {
        let close: () -> Void

        func makeBody(configuration: Configuration) -> some View {
            Button(role: configuration.role) {
                close()
                configuration.trigger()
            } label: {
                configuration.label.textCase(.lowercase)
            }
            .buttonStyle(TelevisionRow())
        }
    }
}
#endif

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

    /// The app is about to quit.
    static var appTerminates: Notification.Name {
        #if canImport(AppKit)
        NSApplication.willTerminateNotification
        #else
        UIApplication.willTerminateNotification
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
    static var secondaryLabel: NSColor { secondaryLabelColor }
    static var quaternaryLabel: NSColor { quaternaryLabelColor }
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

extension View {
    /// A row that gives up its own ground so the wash shows through it.
    ///
    /// An AppKit row is already clear. A UIKit one paints the system background
    /// behind every row, which on a phone puts a black band across the record's
    /// colour wherever there is a list. Row backgrounds are set per row and a
    /// `List` does not pass one down, so this goes on a list's content (a
    /// `Group`, `Section` or `ForEach` reaches every row inside it), not on the
    /// list. In the theme rows have no rules: rhythm and alignment tell them
    /// apart.
    @ViewBuilder
    func washedRow() -> some View {
        #if os(macOS)
        self
        #elseif os(tvOS)
        // A television's list draws no rules.
        listRowBackground(Color.clear)
        #else
        if KoanTheme.isOn {
            listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
        } else {
            listRowBackground(Color.clear)
        }
        #endif
    }
}

#if !os(macOS)
/// A switch in the system's green, whatever the tint.
///
/// A switch that is on draws its track in the tint, and koan's tint is the
/// playing record's colour or, with nothing playing, the primary text colour:
/// white, in dark mode, under a white knob.
struct SystemSwitch: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        #if os(tvOS)
        // A television's toggle is a row that says On or Off. Its label takes
        // the tint, which the room sets to the record's colour; the primary
        // colour lets a focused row draw it dark on white as other rows do.
        // The theme's is its square box.
        if KoanTheme.isOn {
            KoanToggleStyle().makeBody(configuration: configuration)
        } else {
            Toggle(configuration)
                .tint(.primary)
        }
        #else
        Toggle(configuration)
            .toggleStyle(.switch)
            .tint(.green)
        #endif
    }
}
#endif

extension View {
    /// `onHover`, which tvOS does not have: nothing hovers under a remote.
    func pointerHover(perform action: @escaping (Bool) -> Void) -> some View {
        #if os(tvOS)
        self
        #else
        onHover(perform: action)
        #endif
    }

    /// The inset list, or the plain one on tvOS, which has no inset style.
    func insetList() -> some View {
        #if os(tvOS)
        listStyle(.plain)
        #else
        listStyle(.inset)
        #endif
    }

    /// `navigationSubtitle`, which tvOS does not have: its tab pages carry no
    /// title bar to put one under.
    func pageSubtitle(_ subtitle: String) -> some View {
        #if os(tvOS)
        self
        #else
        navigationSubtitle(subtitle)
        #endif
    }
}

/// Drag and drop, text selection and row separators, none of which tvOS has.
/// On tvOS each leaves the view as it is.
extension View {
    func dragSource<T: Transferable>(_ payload: @autoclosure @escaping () -> T) -> some View {
        #if os(tvOS)
        self
        #else
        draggable(payload())
        #endif
    }

    func dropTarget<T: Transferable>(
        for type: T.Type,
        action: @escaping ([T], CGPoint) -> Bool,
        isTargeted: @escaping (Bool) -> Void = { _ in }
    ) -> some View {
        #if os(tvOS)
        self
        #else
        dropDestination(for: type, action: action, isTargeted: isTargeted)
        #endif
    }

    func rowSeparator(_ visibility: Visibility) -> some View {
        #if os(tvOS)
        self
        #else
        listRowSeparator(visibility)
        #endif
    }

    func selectableText() -> some View {
        #if os(tvOS)
        self
        #else
        textSelection(.enabled)
        #endif
    }

    /// The grabber on a sheet, which a remote has no use for.
    func sheetGrabber() -> some View {
        #if os(tvOS)
        self
        #else
        presentationDragIndicator(.visible)
        #endif
    }
}

#if os(iOS) || os(tvOS)
extension View {
    /// A short choice that rises from the bottom: the device trays. The
    /// system's sheet, at half height and up, except on a phone in the theme,
    /// where iOS draws a half-height sheet as a rounded card inset from the
    /// screen's edges whatever its corner radius is told: there it is a
    /// square panel on `bg` across the screen, under a rule, over the page
    /// dimmed, as tall as its content and scrolling past a limit.
    @ViewBuilder
    func tray<Tray: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Tray) -> some View {
        #if os(iOS)
        if KoanTheme.isOn {
            modifier(PhoneTray(isPresented: isPresented, tray: content))
        } else {
            systemTray(isPresented: isPresented, content: content)
        }
        #else
        systemTray(isPresented: isPresented, content: content)
        #endif
    }

    private func systemTray<Tray: View>(isPresented: Binding<Bool>, content: @escaping () -> Tray) -> some View {
        sheet(isPresented: isPresented) {
            ScrollView { content() }
                .koanSheet()
                .presentationDetents([.medium, .large])
                .sheetGrabber()
        }
    }
}
#endif

#if os(iOS)
/// A cover with nothing of its own to draw, presented and dismissed without
/// the system's slide: the panel moves itself, and the dim fades.
private struct PhoneTray<Tray: View>: ViewModifier {
    @Binding var isPresented: Bool
    @ViewBuilder let tray: () -> Tray
    @State private var covering = false

    func body(content: Content) -> some View {
        content
            .onChange(of: isPresented, initial: true) { _, open in
                var instant = Transaction()
                instant.disablesAnimations = true
                withTransaction(instant) { covering = open }
            }
            .fullScreenCover(isPresented: $covering, onDismiss: { isPresented = false }) {
                PhoneTrayPanel(close: { isPresented = false }, content: tray)
                    .presentationBackground(.clear)
            }
    }
}

private struct PhoneTrayPanel<Tray: View>: View {
    let close: () -> Void
    @ViewBuilder let content: () -> Tray
    @State private var shown = false
    /// The content's own height: the panel stands as tall as it, to a limit.
    @State private var height: CGFloat = 0
    /// How far a drag has pulled the panel down; back to nothing when the
    /// drag ends or the system cancels it.
    @GestureState private var pull: CGFloat = 0
    /// Whether the content is scrolled to its top, where a downward drag
    /// pulls the panel instead.
    @State private var atTop = true
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { proxy in
            let below = proxy.size.height + proxy.safeAreaInsets.bottom
            ZStack(alignment: .bottom) {
                Color.black.opacity(shown ? 0.4 : 0)
                    .ignoresSafeArea()
                    .onTapGesture(perform: dismiss)
                    .accessibilityHidden(true)
                panel(limit: proxy.size.height * 0.85)
                    .offset(y: shown ? pull : below)
                    .koanAnimation(KoanTheme.Motion.settle, value: pull == 0)
            }
        }
        .onAppear { animate { shown = true } }
    }

    private func panel(limit: CGFloat) -> some View {
        let compact = sizeClass != .regular
        return VStack(spacing: 0) {
            // The grabber, in a strip across the panel that takes a drag
            // whether or not the content scrolls.
            Rectangle()
                .fill(Color.koanRule)
                .frame(width: 36, height: 3)
                .padding(.vertical, KoanTheme.Space.s)
                .frame(maxWidth: .infinity)
                .contentShape(Rectangle())
                .accessibilityHidden(true)
            ScrollView {
                content().onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
            }
            .scrollBounceBehavior(.basedOnSize)
            // Content that fits has nothing to scroll, and a scroll view that
            // could would take the drag that closes the panel.
            .scrollDisabled(height <= limit)
            .onScrollGeometryChange(for: Bool.self) {
                $0.contentOffset.y <= -$0.contentInsets.top + 0.5
            } action: { _, top in
                atTop = top
            }
            .frame(height: min(max(height, 1), limit))
        }
        .font(.koan(.body))
        .foregroundStyle(Color.koanInk)
        .frame(maxWidth: compact ? .infinity : 560)
        .background(Color.koanBg.ignoresSafeArea(edges: .bottom))
        .overlay(alignment: .top) {
            Rectangle().fill(Color.koanRule).frame(height: KoanTheme.hairline)
        }
        .overlay {
            if !compact {
                Rectangle()
                    .strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline)
                    .ignoresSafeArea(edges: .bottom)
            }
        }
        .simultaneousGesture(
            // In the screen's space: the panel moves under the finger, and its
            // own space would move with it.
            DragGesture(minimumDistance: 12, coordinateSpace: .global)
                .updating($pull) { drag, pull, _ in
                    if atTop { pull = max(0, drag.translation.height) }
                }
                .onEnded { drag in
                    guard atTop else { return }
                    if drag.translation.height > 80 || drag.predictedEndTranslation.height > 240 {
                        dismiss()
                    }
                },
            including: .all
        )
        .accessibilityAddTraits(.isModal)
        .accessibilityAction(.escape, dismiss)
    }

    private func dismiss() {
        animate { shown = false } completion: { close() }
    }

    private func animate(_ change: () -> Void, completion: @escaping () -> Void = {}) {
        if reduceMotion {
            change()
            completion()
        } else {
            withAnimation(KoanTheme.Motion.settle, change, completion: completion)
        }
    }
}
#endif

extension View {
    /// The system file picker, which tvOS does not have: there are no files to
    /// pick on a television.
    func filePicker(
        isPresented: Binding<Bool>,
        allowedContentTypes: [UTType],
        allowsMultipleSelection: Bool,
        onCompletion: @escaping (Result<[URL], any Error>) -> Void
    ) -> some View {
        #if os(tvOS)
        self
        #else
        fileImporter(
            isPresented: isPresented,
            allowedContentTypes: allowedContentTypes,
            allowsMultipleSelection: allowsMultipleSelection,
            onCompletion: onCompletion
        )
        #endif
    }
}

extension View {
    /// The bordered text field, or the system's own on tvOS, which has no
    /// rounded-border style.
    func borderedField() -> some View {
        #if os(tvOS)
        textFieldStyle(.automatic)
        #else
        textFieldStyle(.roundedBorder)
        #endif
    }
}

/// Keyboard shortcuts, which tvOS does not have.
extension View {
    func shortcut(_ key: KeyEquivalent, modifiers: EventModifiers = .command) -> some View {
        #if os(tvOS)
        self
        #else
        keyboardShortcut(key, modifiers: modifiers)
        #endif
    }

    /// A sheet's default or cancel button. Its own type, because tvOS does not
    /// have `KeyboardShortcut` either.
    func shortcut(_ role: ShortcutRole?) -> some View {
        #if os(tvOS)
        self
        #else
        keyboardShortcut(role.map { $0 == .defaultAction ? .defaultAction : .cancelAction })
        #endif
    }
}

enum ShortcutRole {
    case defaultAction, cancelAction
}

#if os(tvOS)
/// A button as a television draws one: a white label on a soft pill at rest,
/// and focused, a white platter with the label drawn as on a light screen.
/// The system's own style takes the label's colour from the tint, which the
/// app sets to its accent and the room to the record's colour, so a label
/// could be mint on grey at rest and vanish into a tinted platter on focus.
/// In the theme, the label alone in `ink`, and focus the accent's ring: no
/// pill, platter, lift or shadow.
struct TelevisionButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Pill(label: configuration.label, pressed: configuration.isPressed)
    }

    private struct Pill<Label: View>: View {
        let label: Label
        let pressed: Bool
        @Environment(\.isFocused) private var focused
        @Environment(\.isEnabled) private var enabled

        var body: some View {
            if KoanTheme.isOn {
                label
                    .foregroundStyle(Color.koanInk)
                    .padding(.horizontal, KoanTheme.Space.l)
                    .padding(.vertical, KoanTheme.Space.m)
                    .background(pressed ? Color.koanHover : .clear)
                    .opacity(enabled ? 1 : 0.4)
                    .koanFocusRing(focused, gap: 0)
            } else {
                label
                    .foregroundStyle(Color.primary)
                    .environment(\.colorScheme, focused ? .light : .dark)
                    .padding(.horizontal, 28)
                    .padding(.vertical, 14)
                    .background(
                        Capsule().fill(.white.opacity(focused ? 1 : 0.14))
                            .shadow(color: .black.opacity(focused ? 0.35 : 0), radius: 18, y: 8)
                    )
                    .opacity(enabled ? 1 : 0.45)
                    .scaleEffect(pressed ? 0.97 : focused ? 1.06 : 1)
                    .animation(.easeOut(duration: 0.15), value: focused)
            }
        }
    }
}

extension View {
    /// The theme's sheet on a television: a square panel on `bg` inside a
    /// rule, over the page dimmed. A cover rather than a sheet, which tvOS
    /// draws as a rounded card whatever its background is told; Menu closes
    /// it as it closes a sheet. It stands as tall as its content, and scrolls
    /// past a limit.
    func televisionPanel<Panel: View>(
        isPresented: Binding<Bool>,
        title: String? = nil,
        @ViewBuilder content: @escaping () -> Panel
    ) -> some View {
        fullScreenCover(isPresented: isPresented) {
            TelevisionPanel(title: title, content: content)
        }
    }
}

private struct TelevisionPanel<Panel: View>: View {
    let title: String?
    @ViewBuilder let content: () -> Panel
    /// The content's own height: the panel stands as tall as it, to a limit.
    @State private var height: CGFloat = 0

    var body: some View {
        ScrollView {
            stack.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .frame(width: 960, height: min(max(height, 1), 880))
        .background(Color.koanBg)
        .overlay { Rectangle().strokeBorder(Color.koanRule, lineWidth: KoanTheme.hairline) }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .presentationBackground(Color.black.opacity(0.6))
    }

    private var stack: some View {
        VStack(alignment: .leading, spacing: KoanTheme.Space.xs) {
            if let title {
                Text(title)
                    .koanText(.titleSmall, .strong)
                    .textCase(.lowercase)
                    .padding(.horizontal, 20)
                    .padding(.bottom, KoanTheme.Space.m)
            }
            content()
        }
        .font(.koan(.body))
        .foregroundStyle(Color.koanInk)
        .padding(KoanTheme.Space.xxl)
    }
}

/// A list row as a television draws one: the row's own colours at rest, and
/// focused, a white platter with the row drawn as it would be on a light
/// screen, so secondary text stays readable on it. A plain button would tint
/// every label with the accent instead. In the theme, the row on the ground
/// as it is, and focus the accent's ring around it.
struct TelevisionRow: ButtonStyle {
    /// The platter's opacity at rest: none for a row of content, a little for
    /// a link, so a list of places reads as rows before one is focused. The
    /// theme draws none.
    var resting: Double = 0

    func makeBody(configuration: Configuration) -> some View {
        Row(label: configuration.label, pressed: configuration.isPressed, resting: resting)
    }

    private struct Row<Label: View>: View {
        let label: Label
        let pressed: Bool
        let resting: Double
        @Environment(\.isFocused) private var focused
        @Environment(\.isEnabled) private var enabled
        @Environment(\.koanRowsBleed) private var bleeds

        var body: some View {
            if KoanTheme.isOn {
                // In a form the row's text keeps the headings' edge, and the
                // ring stands out into the margin.
                label
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .foregroundStyle(Color.koanInk)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 6)
                    .background(pressed ? Color.koanHover : .clear)
                    .opacity(enabled ? 1 : 0.4)
                    .koanFocusRing(focused, gap: 0)
                    .padding(.horizontal, bleeds ? -20 : 0)
            } else {
                label
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .foregroundStyle(Color.primary)
                    .environment(\.colorScheme, focused ? .light : .dark)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 6)
                    .background(
                        RoundedRectangle(cornerRadius: KoanTheme.radius(14))
                            .fill(.white.opacity(focused ? 1 : resting))
                            .shadow(color: .black.opacity(focused ? 0.35 : 0), radius: 18, y: 8)
                    )
                    .scaleEffect(pressed ? 0.98 : focused ? 1.02 : 1)
                    .animation(.easeOut(duration: 0.15), value: focused)
            }
        }
    }
}
#endif
