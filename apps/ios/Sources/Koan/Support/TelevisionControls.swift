import SwiftUI
import UIKit

extension View {
    /// A field holding a machine value — a URL, an account name, a format
    /// string — rather than prose. The system keyboard assumes prose: it
    /// capitalises the first letter and autocorrects as it goes, which turns
    /// `https://music.blit.cc` into `Https://music.blit.cc` and ruins a
    /// password.
    func verbatimEntry(_ contentType: VerbatimContent = .plain) -> some View {
        autocorrectionDisabled()
            .textInputAutocapitalization(contentType == .code ? .characters : .never)
            .keyboardType(contentType.keyboard)
    }
}

/// What kind of machine value, for the keyboard the system should offer.
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
    /// A button or menu in a toolbar. It takes the system's toolbar style back
    /// from the shell's `TelevisionButton`, which would draw it as a capsule
    /// with its symbol at text size, and shows the symbol alone: a
    /// television's toolbar truncates a title to a letter or two.
    func toolbarButton() -> some View {
        buttonStyle(.automatic).labelStyle(.iconOnly)
    }

    /// A `NavigationLink` in a list, drawn as a full-width row: the shell's
    /// button style would otherwise make it a capsule the size of its label.
    func listLink() -> some View {
        buttonStyle(TelevisionRow(resting: 0.08))
    }

    /// A list row's primary action, such as opening the record or playing the
    /// track, and its menu. The row is a button, and a long press of the
    /// remote opens the menu: in the theme a panel of the theme's rows,
    /// otherwise the system's context menu.
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

/// A toggle as a television draws one: a row that says On or Off. Its label
/// takes the tint, which the room sets to the record's colour; the primary
/// colour lets a focused row draw it dark on white as other rows do. In the
/// theme, the theme's square box.
struct TelevisionToggle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        if KoanTheme.isOn {
            KoanToggleStyle().makeBody(configuration: configuration)
        } else {
            Toggle(configuration)
                .tint(.primary)
        }
    }
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
