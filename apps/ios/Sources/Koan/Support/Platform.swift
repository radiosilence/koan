import SwiftUI
import UniformTypeIdentifiers
import UIKit

/// The handful of places where AppKit and UIKit disagree about a type koan
/// uses.
///
/// Everything else in the app is SwiftUI and crosses on its own. This exists so
/// the art pipeline — which is `CGImageSource` end to end and only meets a
/// platform image at the very last step — doesn't have to be written twice.
typealias PlatformImage = UIImage

extension PlatformImage {
    /// Wrap a decoded bitmap without redrawing it.
    ///
    /// The size is in pixels, which is what the decode produced: AppKit wants it
    /// stated, UIKit infers it from the `CGImage` and ignores what it is told.
    static func decoded(_ bitmap: CGImage, pixelSize: CGSize) -> PlatformImage {
        UIImage(cgImage: bitmap)
    }

    /// The bitmap behind the image, for sampling rather than drawing.
    var bitmap: CGImage? {
        return cgImage
    }
}

extension Image {
    init(platform: PlatformImage) {
        self.init(uiImage: platform)
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
        autocorrectionDisabled()
            .textInputAutocapitalization(contentType == .code ? .characters : .never)
            .keyboardType(contentType.keyboard)
    }
}

/// What kind of machine value, for the keyboard iOS should offer.
enum VerbatimContent {
    case plain
    case url
    case digits
    /// A code typed in capitals, such as a pairing code.
    case code

    var keyboard: UIKeyboardType {
        switch self {
        case .plain, .code: .asciiCapable
        case .url: .URL
        case .digits: .numberPad
        }
    }
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
        buttonStyle(TelevisionButton())
    }
}

extension View {
    /// At least 44 points square to a finger, Apple's minimum, however small
    /// the glyph drawn. A pointer needs no more than the glyph, and a
    /// television focuses rather than hits.
    func touchTarget() -> some View {
        self
    }

    /// A control drawn as its glyph alone on the Mac and the phone. A
    /// television gives it the round platter its neighbours have: a bare glyph
    /// shows no focus, and cannot be found from across the room.
    func controlButton() -> some View {
        buttonStyle(TelevisionButton())
    }

    /// A button or menu in a toolbar. On tvOS it takes the system's toolbar
    /// style back from the shell's `TelevisionButton`, which would draw it as a
    /// capsule with its symbol at text size, and shows the symbol alone: a
    /// television's toolbar truncates a title to a letter or two.
    func toolbarButton() -> some View {
        buttonStyle(.automatic).labelStyle(.iconOnly)
    }

    /// A `NavigationLink` in a list. On tvOS it is drawn as a full-width row:
    /// the shell's button style would otherwise make it a capsule the size of
    /// its label. Elsewhere it is a row on the wash.
    func listLink() -> some View {
        buttonStyle(TelevisionRow(resting: 0.08))
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
        Button(action: action) { contentShape(Rectangle()) }
            .buttonStyle(TelevisionRow())
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
        self
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
        if KoanTheme.isOn {
            modifier(TelevisionMenuRow(action: action, menu: menu))
        } else {
            Button(action: action) { contentShape(Rectangle()) }
                .buttonStyle(TelevisionRow())
                .contextMenu { menu() }
        }
    }
}

/// A row whose menu, in the theme, is a sheet of the theme's rows rather than
/// the system's popover of grey pills. A long press of the remote opens it,
/// as it does the system's. Focusable rather than a button: a button takes
/// the press for itself, and a long one never reaches the gesture.
private struct TelevisionMenuRow<Menu: View>: ViewModifier {
    let action: () -> Void
    @ViewBuilder let menu: () -> Menu
    @Environment(PlayerModel.self) private var player: PlayerModel?
    @State private var open = false
    /// An item was chosen, and focus has not yet come back from the menu.
    @State private var chose = false
    /// A share code this row's menu asked for is up: the shell shows it, and
    /// tvOS gives focus back to the page's first control when it closes.
    @State private var sharing = false
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
                menu().buttonStyle(MenuItem(close: {
                    chose = true
                    open = false
                }))
            }
            // Focus back on the row, or gone elsewhere: either way the menu's
            // choice no longer stands for it.
            .onChange(of: focused) { if !open { chose = false } }
            .onDisappear { chose = false }
            .onChange(of: player?.sharedLink != nil) { _, shown in
                if shown {
                    sharing = focused || chose
                    chose = false
                } else if sharing {
                    sharing = false
                    focused = true
                }
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

extension Notification.Name {
    /// The app giving up the foreground — on iOS the last dependable moment
    /// before it is suspended and perhaps killed without another word.
    static var appResignsActive: Notification.Name {
        UIApplication.willResignActiveNotification
    }

    /// The app taking the foreground again.
    static var appBecomesActive: Notification.Name {
        UIApplication.didBecomeActiveNotification
    }

    /// The app is about to quit.
    static var appTerminates: Notification.Name {
        UIApplication.willTerminateNotification
    }
}

typealias PlatformColor = UIColor

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
        layer
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (view: LayerView, _) in
            view.appearanceChanged()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not from a nib") }

    /// The size may have changed.
    func layoutLayers() {}

    /// Light and dark swapped; anything holding a resolved `CGColor` repaints.
    func appearanceChanged() {}

    /// A dynamic colour pinned to this view's current appearance.
    func resolved(_ colour: PlatformColor) -> CGColor {
        colour.resolvedColor(with: traitCollection).cgColor
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        layoutLayers()
    }
}

typealias PlatformView = UIView

/// `NSViewRepresentable` and `UIViewRepresentable` under one set of names.
///
/// Conformers name `PlatformViewType` outright. Inferring it through the
/// platform's own associated type works for some and not others, depending on
/// the order the compiler happens to meet them in.
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

extension View {
    /// A button that reads as a link. iOS has no link style; a borderless
    /// button in the tint is what it uses for the same job.
    func linkButton() -> some View {
        buttonStyle(.borderless)
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
        // A television's list draws no rules.
        listRowBackground(Color.clear)
    }
}

/// A switch in the system's green, whatever the tint.
///
/// A switch that is on draws its track in the tint, and koan's tint is the
/// playing record's colour or, with nothing playing, the primary text colour:
/// white, in dark mode, under a white knob.
struct SystemSwitch: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
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
    }
}

extension View {
    /// `onHover`, which tvOS does not have: nothing hovers under a remote.
    func pointerHover(perform action: @escaping (Bool) -> Void) -> some View {
        self
    }

    /// The inset list, or the plain one on tvOS, which has no inset style.
    func insetList() -> some View {
        listStyle(.plain)
    }

    /// `navigationSubtitle`, which tvOS does not have: its tab pages carry no
    /// title bar to put one under.
    func pageSubtitle(_ subtitle: String) -> some View {
        self
    }
}

/// Drag and drop, text selection and row separators, none of which tvOS has.
/// On tvOS each leaves the view as it is.
extension View {
    func dragSource<T: Transferable>(_ payload: @autoclosure @escaping () -> T) -> some View {
        self
    }

    func dropTarget<T: Transferable>(
        for type: T.Type,
        action: @escaping ([T], CGPoint) -> Bool,
        isTargeted: @escaping (Bool) -> Void = { _ in }
    ) -> some View {
        self
    }

    func rowSeparator(_ visibility: Visibility) -> some View {
        self
    }

    func selectableText() -> some View {
        self
    }

    /// The grabber on a sheet, which a remote has no use for.
    func sheetGrabber() -> some View {
        self
    }
}

extension View {
    /// A short choice that rises from the bottom: the device trays. The
    /// system's sheet, at half height and up, except on a phone in the theme,
    /// where iOS draws a half-height sheet as a rounded card inset from the
    /// screen's edges whatever its corner radius is told: there it is a
    /// square panel on `bg` across the screen, under a rule, over the page
    /// dimmed, as tall as its content and scrolling past a limit.
    @ViewBuilder
    func tray<Tray: View>(
        isPresented: Binding<Bool>,
        onDismiss: (() -> Void)? = nil,
        @ViewBuilder content: @escaping () -> Tray
    ) -> some View {
        systemTray(isPresented: isPresented, onDismiss: onDismiss, content: content)
    }

    private func systemTray<Tray: View>(
        isPresented: Binding<Bool>,
        onDismiss: (() -> Void)?,
        content: @escaping () -> Tray
    ) -> some View {
        sheet(isPresented: isPresented, onDismiss: onDismiss) {
            ScrollView { content() }
                .koanSheet()
                .presentationDetents([.medium, .large])
                .sheetGrabber()
        }
    }
}

extension View {
    /// A choice too long for a tray: a form in its own stack, with its Done
    /// in the bar. The system's sheet, except on a phone in the theme, where
    /// iOS draws a sheet over a sheet as a rounded card: there it is the
    /// tray's square panel, at its full height. Either way in the theme's
    /// sheet chrome, which the content leaves to this.
    @ViewBuilder
    func formTray<Item: Identifiable, Tray: View>(
        item: Binding<Item?>,
        onDismiss: (() -> Void)? = nil,
        @ViewBuilder content: @escaping (Item) -> Tray
    ) -> some View {
        sheet(item: item, onDismiss: onDismiss) { content($0).koanSheet() }
    }

    @ViewBuilder
    func formTray<Tray: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Tray) -> some View {
        sheet(isPresented: isPresented) { content().koanSheet() }
    }
}


extension View {
    /// The system file picker, which tvOS does not have: there are no files to
    /// pick on a television.
    func filePicker(
        isPresented: Binding<Bool>,
        allowedContentTypes: [UTType],
        allowsMultipleSelection: Bool,
        onCompletion: @escaping (Result<[URL], any Error>) -> Void
    ) -> some View {
        self
    }
}

extension View {
    /// The bordered text field, or the system's own on tvOS, which has no
    /// rounded-border style.
    @ViewBuilder
    func borderedField() -> some View {
        textFieldStyle(.automatic)
    }
}

/// Keyboard shortcuts, which tvOS does not have.
extension View {
    func shortcut(_ key: KeyEquivalent, modifiers: EventModifiers = .command) -> some View {
        self
    }

    /// A sheet's default or cancel button. Its own type, because tvOS does not
    /// have `KeyboardShortcut` either.
    func shortcut(_ role: ShortcutRole?) -> some View {
        self
    }
}

enum ShortcutRole {
    case defaultAction, cancelAction
}

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
    /// At the limit until measured: a panel drawn shorter than its content
    /// has nothing focusable in it, and focus stays on the page behind.
    @State private var height: CGFloat = 880

    var body: some View {
        ScrollView {
            stack.onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .scrollBounceBehavior(.basedOnSize)
        .focusSection()
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

/// A button whose label draws itself, such as a pill: only focus is added.
/// In the theme the accent's ring; in the platform's look the lift every
/// television control has.
struct TelevisionChip: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Chip(label: configuration.label, pressed: configuration.isPressed)
    }

    private struct Chip<Label: View>: View {
        let label: Label
        let pressed: Bool
        @Environment(\.isFocused) private var focused

        var body: some View {
            if KoanTheme.isOn {
                label
                    .foregroundStyle(Color.koanInk)
                    .background(pressed ? Color.koanHover : .clear)
                    .koanFocusRing(focused, gap: 4)
            } else {
                label
                    .foregroundStyle(Color.primary)
                    .environment(\.colorScheme, focused ? .light : .dark)
                    .background(Capsule().fill(.white.opacity(focused ? 1 : 0)))
                    .shadow(color: .black.opacity(focused ? 0.35 : 0), radius: 18, y: 8)
                    .scaleEffect(pressed ? 0.97 : focused ? 1.08 : 1)
                    .animation(.easeOut(duration: 0.15), value: focused)
            }
        }
    }
}

extension View {
    /// A tap a remote can make too. On a television only what takes focus can
    /// be clicked, so there the view becomes a button's label.
    @ViewBuilder
    func tapAction(_ action: @escaping () -> Void) -> some View {
        Button(action: action) { self }
            .buttonStyle(TelevisionChip())
    }
}

