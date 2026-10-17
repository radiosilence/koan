import SwiftUI

/// The head of a page: a cover if the page has one, then the title with any
/// play controls on its line, a line of detail under it — a count, a credit —
/// and the page's lesser actions below that. Whatever acts on the page as a
/// whole trails.
///
/// One layout for records, artists, playlists and the library's own pages, so
/// the title, its count and its actions are in the same places wherever you
/// are ("Shared edges" in `docs/design/koan-theme.md`). Top-aligned: the
/// cover's top meets the title's first line. Wider layouts only; a phone's
/// stacked headers are their pages' own.
struct PageHeader<Art: View, Lead: View, Detail: View, Actions: View, Trailing: View>: View {
    let title: String
    /// The app's own word ("Tracks") rather than data (a record's name), so it
    /// takes the theme's case.
    let appWords: Bool
    let role: KoanType
    let systemSize: CGFloat
    let art: Art
    let lead: Lead
    let detail: Detail
    let actions: Actions
    let trailing: Trailing

    init(
        _ title: String,
        appWords: Bool = false,
        role: KoanType = .title,
        systemSize: CGFloat = pageTitleSize,
        @ViewBuilder art: () -> Art = { EmptyView() },
        @ViewBuilder lead: () -> Lead = { EmptyView() },
        @ViewBuilder detail: () -> Detail = { EmptyView() },
        @ViewBuilder actions: () -> Actions = { EmptyView() },
        @ViewBuilder trailing: () -> Trailing = { EmptyView() }
    ) {
        self.title = title
        self.appWords = appWords
        self.role = role
        self.systemSize = systemSize
        self.art = art()
        self.lead = lead()
        self.detail = detail()
        self.actions = actions()
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .top, spacing: 18) {
            art
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .headerCentre, spacing: pageTitleGap) {
                    lead
                    Text(title)
                        .textCase(appWords && KoanTheme.isOn ? .lowercase : nil)
                        .font(.role(role, system: .system(size: systemSize, weight: .semibold)))
                        .headerTitleCentre(role, systemSize: systemSize)
                        .foregroundStyle(KoanTheme.style(.strong, system: .primary))
                        .lineLimit(2)
                        // Beside the play button the row offers one line's
                        // height; asked for two, it has to be let grow.
                        .fixedSize(horizontal: false, vertical: true)
                }
                if Detail.self != EmptyView.self {
                    detail
                        .font(.role(.control, system: .callout))
                        .foregroundStyle(KoanTheme.style(.muted, system: .secondary))
                }
                if Actions.self != EmptyView.self {
                    actions.padding(.top, 4)
                }
            }
            Spacer(minLength: 0)
            trailing
        }
    }
}

#if os(tvOS)
let pageTitleSize: CGFloat = 48
private let pageTitleGap: CGFloat = 32
#else
let pageTitleSize: CGFloat = 26
private let pageTitleGap: CGFloat = 12
#endif
