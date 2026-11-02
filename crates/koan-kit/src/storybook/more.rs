//! The storybook's pages for containers and moving parts: collections,
//! tables and lists, panels and sheets, motion and drag.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Story, StoryContent, text};
use crate::components::{
    Align, BarLevels, Collection, CollectionSection, CollectionWidth, Column, CopyButton, CopyText,
    CoverArt, DenseRow, DenseTable, DetailGroup, DragChip, DropTarget, EmptyState, FadeIn,
    FilterField, Form, FormRow, FormSection, Inspector, KoanContextMenu, MenuEntry, Mosaic,
    NavHeading, NavNote, NavRow, PageHeader, Pill, PlayButton, PlayingBars, QrCode, Reveal,
    RowKind, RowList, RowListState, SectionHeader, SectionHeading, ShuffleButton, SplitView,
    SuggestionHeading, SuggestionPanel, SuggestionRow, SyncedLines, TableRow, TileArt, TileGrid,
    TileSleeve, Viewer, link_action, link_text, name_line, plain_text, table_list,
};
use crate::theme::{HAIRLINE, Icon, Look, Space, Tone, TypeRole};

/// What the components on these pages hold.
pub struct State {
    collection: CollectionWidth,
    filter: Entity<InputState>,
    rows: RowListState,
    split: Pixels,
    line: usize,
    picked: [bool; 3],
    arriving: bool,
    dropped: usize,
    viewing: bool,
    bars: BarLevels,
    qr: QrCode,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<StoryContent>) -> Self {
        let rows = RowListState::new();
        rows.reset(LIST.len() * 4, RowKind::Text);
        let bars = BarLevels::new();
        bars.set([0.7, 0.45, 0.25]);
        Self {
            collection: CollectionWidth::default(),
            filter: cx.new(|cx| InputState::new(window, cx).default_value("low")),
            rows,
            split: px(220.0),
            line: 2,
            picked: [true, false, false],
            arriving: true,
            dropped: 0,
            viewing: false,
            bars,
            qr: sample_code(),
        }
    }
}

/// A drag of three tracks, as the storybook's drop target expects.
struct Dragged;

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        DragChip::new("3 tracks").sleeve(CoverArt::Colour(WARM))
    }
}

const WARM: Hsla = Hsla {
    h: 0.05,
    s: 0.6,
    l: 0.45,
    a: 1.0,
};
const COOL: Hsla = Hsla {
    h: 0.55,
    s: 0.5,
    l: 0.4,
    a: 1.0,
};

const LIST: [(&str, &str, &str); 3] = [
    ("Shoreline Pinball", "Low Tide Arcade", "3:41"),
    ("Coin Return", "Low Tide Arcade", "4:02"),
    ("Neon Breakwater", "Low Tide Arcade", "5:17"),
];

const LYRICS: [&str; 6] = [
    "the tide pulls the lights out",
    "one coin at a time",
    "we kept the high score",
    "under the boardwalk",
    "and the sea took the rest",
    "♪",
];

impl StoryContent {
    fn update(
        entity: &Entity<StoryContent>,
        change: impl Fn(&mut StoryContent) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = entity.clone();
        move |_, _, cx| {
            entity.update(cx, |story, cx| {
                change(story);
                cx.notify();
            })
        }
    }

    pub(super) fn more(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let entity = cx.entity();
        let frame = |height: f32| {
            div()
                .h(look.spaced(height))
                .border(HAIRLINE)
                .border_color(look.palette.rule.hsla())
        };
        match self.story {
            Story::Collections => {
                let grid = self
                    .more
                    .collection
                    .grid(look)
                    .unwrap_or_else(|| TileGrid::for_width(look.spaced(800.0), &look.scale));
                let picked = self.more.picked;
                let pills = ["Low Tide Arcade", "Polar Bear", "Phace"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| {
                        Pill::new(("story-pill", index), Icon::Artist, name)
                            .ticking(picked[index])
                            .on_click(Self::update(&entity, move |story| {
                                story.more.picked[index] = !story.more.picked[index];
                            }))
                    });
                let tiles = [WARM, COOL, WARM]
                    .into_iter()
                    .enumerate()
                    .map(|(index, colour)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.space(Space::Xs))
                            .child(TileArt::new(grid.side, TileSleeve::Colour(colour)))
                            .child(text(
                                ["Low Tide Arcade", "Breakwater", "Coin Return"][index],
                                TypeRole::Control,
                                Tone::Ink,
                                look,
                            ))
                            .into_any_element()
                    });
                let rows = LIST
                    .into_iter()
                    .enumerate()
                    .map(|(index, (title, artist, length))| {
                        TableRow::new(("story-collection-row", index), RowKind::Text)
                            .title(text(title, TypeRole::Body, Tone::Ink, look))
                            .line(text(artist, TypeRole::Meta, Tone::Muted, look))
                            .figure(48.0, text(length, TypeRole::Meta, Tone::Muted, look))
                            .into_any_element()
                    });
                vec![
                    SectionHeader::new("Page header with play and shuffle").into_any_element(),
                    PageHeader::new("Low Tide Arcade")
                        .art(Mosaic::new(
                            vec![
                                CoverArt::Colour(WARM),
                                CoverArt::Colour(COOL),
                                CoverArt::Loading,
                                CoverArt::Missing,
                            ],
                            look.spaced(120.0),
                        ))
                        .detail(text(
                            "12 tracks · 48 min",
                            TypeRole::Control,
                            Tone::Muted,
                            look,
                        ))
                        .action(PlayButton::new("story-play", "play"))
                        .action(ShuffleButton::new("story-shuffle", "Low Tide Arcade"))
                        .action(PlayButton::new("story-play-loading", "play").loading(true))
                        .into_any_element(),
                    SectionHeader::new("A collection: pills, tiles and rows").into_any_element(),
                    frame(520.0)
                        .child(
                            Collection::new("story-collection", &self.more.collection)
                                .section(CollectionSection::pills(
                                    SectionHeading::new("Artists").count("3"),
                                    pills,
                                ))
                                .section(CollectionSection::tiles(
                                    SectionHeading::new("Records")
                                        .count("3")
                                        .on_open("story-records-open", |_, _, _| {}),
                                    grid,
                                    tiles,
                                ))
                                .section(CollectionSection::rows(
                                    SectionHeading::new("Tracks"),
                                    rows,
                                )),
                        )
                        .into_any_element(),
                    SectionHeader::new("An empty collection").into_any_element(),
                    frame(200.0)
                        .child(
                            Collection::new("story-empty", &CollectionWidth::default()).empty(
                                EmptyState::new(Icon::Favourite, "No favourites")
                                    .detail("Hearts put records, artists and tracks here."),
                            ),
                        )
                        .into_any_element(),
                ]
            }
            Story::Lists => {
                let ink = look.palette.ink.hsla();
                vec![
                    SectionHeader::new("Navigation headings and notes").into_any_element(),
                    div()
                        .w(look.spaced(240.0))
                        .flex()
                        .flex_col()
                        .child(NavHeading::new("Playlists"))
                        .child(NavRow::new("story-nav-1", "Sunday morning").icon(Icon::Playlist))
                        .child(NavNote::new("Signed out of your server", Tone::Bad))
                        .child(NavNote::new("12 playlists", Tone::Muted))
                        .into_any_element(),
                    SectionHeader::new("Links").into_any_element(),
                    div()
                        .w(look.spaced(260.0))
                        .child(name_line(
                            vec![
                                link_text(
                                    "story-link-1",
                                    "Low Tide Arcade",
                                    ink,
                                    Some(link_action(|_, _, _| {})),
                                )
                                .into_any_element(),
                                div().child(", ").into_any_element(),
                                link_text(
                                    "story-link-2",
                                    "The Breakwater Collective featuring Coin Return",
                                    ink,
                                    Some(link_action(|_, _, _| {})),
                                )
                                .into_any_element(),
                            ],
                            TypeRole::Body,
                            look.palette.muted.hsla(),
                            look,
                        ))
                        .into_any_element(),
                    SectionHeader::new("Details").into_any_element(),
                    div()
                        .w(look.spaced(360.0))
                        .child(
                            DetailGroup::new("File")
                                .row("Format", "FLAC 24/96")
                                .row("Size", "48.2 MB")
                                .row("Path", "~/Music/Low Tide Arcade/03 Neon Breakwater.flac"),
                        )
                        .into_any_element(),
                    SectionHeader::new("A dense table").into_any_element(),
                    DenseTable::new(vec![
                        Column::fixed(80.0, Align::End),
                        Column::fixed(80.0, Align::End),
                        Column::flex(),
                    ])
                    .header(["Hz", "dB", "Type"])
                    .rows([
                        DenseRow::new().cell("105").cell("+4.5").cell("low shelf"),
                        DenseRow::new().cell("1,200").cell("-2.0").cell("peak"),
                        DenseRow::new()
                            .cell("8,400")
                            .cell("+1.5")
                            .cell("high shelf"),
                    ])
                    .into_any_element(),
                    SectionHeader::new("A virtual table").into_any_element(),
                    frame(180.0)
                        .child(
                            table_list("story-table-list", 40, move |range, _, _| {
                                range
                                    .map(|index| {
                                        TableRow::new(("story-virtual", index), RowKind::Line)
                                            .title(div().child(format!("Row {}", index + 1)))
                                    })
                                    .collect()
                            })
                            .size_full(),
                        )
                        .into_any_element(),
                    SectionHeader::new("Rows measured as drawn").into_any_element(),
                    frame(180.0)
                        .child(RowList::new(&self.more.rows, move |index, _, _| {
                            let (title, artist, _) = LIST[index % LIST.len()];
                            TableRow::new(("story-row-list", index), RowKind::Text)
                                .title(div().child(title))
                                .line(div().child(artist))
                                .into_any_element()
                        }))
                        .into_any_element(),
                ]
            }
            Story::Panels => {
                let split = self.more.split;
                let line = self.more.line;
                let lines = LYRICS
                    .iter()
                    .map(|line| SharedString::from(*line))
                    .collect();
                vec![
                    SectionHeader::new("A form").into_any_element(),
                    frame(240.0)
                        .child(
                            Form::new("story-form")
                                .section(
                                    FormSection::titled("Library")
                                        .row(FormRow::new("Folder").value("~/Music"))
                                        .row(FormRow::new("Watch for changes").value("on"))
                                        .footer("kōan rescans a folder when its files change."),
                                )
                                .section(FormSection::new().row(
                                    FormRow::new("Share link").control(CopyButton::new(
                                        "story-copy",
                                        "copy link",
                                        "https://koan.rocks/s/abc",
                                    )),
                                ))
                                .build(look),
                        )
                        .into_any_element(),
                    SectionHeader::new("Copyable text").into_any_element(),
                    CopyText::new("story-copy-text", "koan.rocks/pair/7KQ2-M9XD")
                        .help("copy the link")
                        .into_any_element(),
                    SectionHeader::new("Filter field and suggestions").into_any_element(),
                    div()
                        .relative()
                        .w(look.spaced(320.0))
                        .h(look.spaced(220.0))
                        .child(
                            div()
                                .relative()
                                .child(FilterField::new(&self.more.filter).fill())
                                .child(
                                    SuggestionPanel::new("story-suggestions")
                                        .child(SuggestionHeading::new("Records"))
                                        .child(
                                            SuggestionRow::new(
                                                "story-suggest-1",
                                                "Low Tide Arcade",
                                            )
                                            .detail("Low Tide Arcade")
                                            .icon(Icon::Album)
                                            .lit(true),
                                        )
                                        .child(
                                            SuggestionRow::new("story-suggest-2", "Lowlands")
                                                .detail("Polar Bear")
                                                .figure("2014")
                                                .icon(Icon::Album),
                                        )
                                        .child(SuggestionHeading::new("Artists"))
                                        .child(
                                            SuggestionRow::new(
                                                "story-suggest-3",
                                                "Low Tide Arcade",
                                            )
                                            .icon(Icon::Artist)
                                            .picked(true),
                                        ),
                                ),
                        )
                        .into_any_element(),
                    SectionHeader::new("Inspector and synced lines").into_any_element(),
                    frame(280.0)
                        .w(look.spaced(320.0))
                        .child(Inspector::new("lyrics").body(
                            SyncedLines::new("story-lines", lines, Some(line)).on_line({
                                let entity = entity.clone();
                                move |index, _, cx| {
                                    let index = *index;
                                    entity.update(cx, |story, cx| {
                                        story.more.line = index;
                                        cx.notify();
                                    })
                                }
                            }),
                        ))
                        .into_any_element(),
                    frame(160.0)
                        .w(look.spaced(320.0))
                        .child(Inspector::new("lyrics").body(plain_text(
                            "Plain lyrics, with no timings,\nas a block of text.",
                            look,
                        )))
                        .into_any_element(),
                    SectionHeader::new("A split view").into_any_element(),
                    frame(160.0)
                        .child(
                            SplitView::new("story-split", split, px(120.0), px(400.0), px(220.0))
                                .leading(div().p(look.space(Space::M)).child("leading"))
                                .trailing(
                                    div()
                                        .p(look.space(Space::M))
                                        .child("trailing; drag the edge"),
                                )
                                .on_resize({
                                    let entity = entity.clone();
                                    move |width, _, cx| {
                                        let width = *width;
                                        entity.update(cx, |story, cx| {
                                            story.more.split = width;
                                            cx.notify();
                                        })
                                    }
                                }),
                        )
                        .into_any_element(),
                    SectionHeader::new("A code to scan").into_any_element(),
                    div()
                        .child(self.more.qr.element(look.spaced(160.0), look.spaced(12.0)))
                        .into_any_element(),
                    SectionHeader::new("The artwork viewer").into_any_element(),
                    crate::components::Button::new(
                        "story-view",
                        crate::components::ButtonKind::Bordered,
                    )
                    .label("open the viewer")
                    .on_click(Self::update(&entity, |story| story.more.viewing = true))
                    .into_any_element(),
                ]
            }
            Story::Motion => {
                let arriving = self.more.arriving;
                let dropped = self.more.dropped;
                let levels = self.more.bars.clone();
                let side = look.spaced(96.0);
                vec![
                    SectionHeader::new("Fade in and reveal").into_any_element(),
                    div()
                        .flex()
                        .gap(look.space(Space::M))
                        .child(FadeIn::new(
                            ("story-fade", arriving as usize),
                            crate::components::Cover::new(CoverArt::Colour(WARM), side),
                        ))
                        .child(
                            Reveal::new(
                                "story-reveal",
                                size(side, side),
                                crate::components::Cover::new(CoverArt::Colour(COOL), side),
                            )
                            .arriving(arriving),
                        )
                        .into_any_element(),
                    crate::components::Button::new(
                        "story-again",
                        crate::components::ButtonKind::Bordered,
                    )
                    .label("again")
                    .on_click(Self::update(&entity, |story| {
                        story.more.arriving = !story.more.arriving
                    }))
                    .into_any_element(),
                    SectionHeader::new("Playing bars").into_any_element(),
                    div()
                        .flex()
                        .items_center()
                        .gap(look.space(Space::S))
                        .child(PlayingBars::new(look.accent.hsla(), move |_, _, _| {
                            Some(levels.clone())
                        }))
                        .child(text(
                            "bands held at 70, 45 and 25 %",
                            TypeRole::Meta,
                            Tone::Muted,
                            look,
                        ))
                        .into_any_element(),
                    SectionHeader::new("Drag and drop").into_any_element(),
                    div()
                        .flex()
                        .gap(look.space(Space::L))
                        .child(
                            div()
                                .id("story-drag")
                                .p(look.space(Space::M))
                                .border(HAIRLINE)
                                .border_color(look.palette.rule.hsla())
                                .cursor_grab()
                                .child("drag me")
                                .on_drag(Dragged, |_, _, _, cx| cx.new(|_| Dragged)),
                        )
                        .child(
                            div()
                                .id("story-drop")
                                .w(look.spaced(220.0))
                                .p(look.space(Space::M))
                                .border(HAIRLINE)
                                .border_color(look.palette.rule.hsla())
                                .drop_highlight::<Dragged>(look)
                                .child(format!("drop here ({dropped} so far)"))
                                .on_drop({
                                    let entity = entity.clone();
                                    move |_: &Dragged, _, cx| {
                                        entity.update(cx, |story, cx| {
                                            story.more.dropped += 1;
                                            cx.notify();
                                        })
                                    }
                                }),
                        )
                        .into_any_element(),
                    SectionHeader::new("A context menu").into_any_element(),
                    div()
                        .id("story-context")
                        .w(look.spaced(220.0))
                        .p(look.space(Space::M))
                        .border(HAIRLINE)
                        .border_color(look.palette.rule.hsla())
                        .child("right-click for a menu")
                        .koan_context_menu("story-context-menu", window, cx, |menu, _, _| {
                            menu.item(
                                MenuEntry::new("play")
                                    .icon(Icon::Play)
                                    .on_click(|_, _, _| {}),
                            )
                            .item(
                                MenuEntry::new("add to queue")
                                    .icon(Icon::QueueSection)
                                    .on_click(|_, _, _| {}),
                            )
                            .item(
                                MenuEntry::new("forget")
                                    .destructive()
                                    .on_click(|_, _, _| {}),
                            )
                        })
                        .into_any_element(),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// The artwork viewer over the whole storybook, while it is open.
    pub(super) fn viewer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entity = cx.entity();
        self.more.viewing.then(|| {
            div()
                .absolute()
                .size_full()
                .child(
                    Viewer::new("story-viewer", CoverArt::Colour(WARM), "Low Tide Arcade")
                        .detail("Low Tide Arcade · 2024")
                        .on_click(Self::update(&entity, |story| story.more.viewing = false)),
                )
                .into_any_element()
        })
    }
}

/// A sample code: the three finder squares and a scatter between them.
fn sample_code() -> QrCode {
    const SIDE: usize = 25;
    let mut dark = vec![false; SIDE * SIDE];
    let finder = |dark: &mut Vec<bool>, top: usize, left: usize| {
        for y in 0..7 {
            for x in 0..7 {
                let ring = x == 0 || y == 0 || x == 6 || y == 6;
                let core = (2..5).contains(&x) && (2..5).contains(&y);
                dark[(top + y) * SIDE + left + x] = ring || core;
            }
        }
    };
    finder(&mut dark, 0, 0);
    finder(&mut dark, 0, SIDE - 7);
    finder(&mut dark, SIDE - 7, 0);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let free = !((y < 8 && !(8..SIDE - 8).contains(&x)) || (y >= SIDE - 8 && x < 8));
            if free && (x * 7 + y * 13 + x * y) % 5 < 2 {
                dark[y * SIDE + x] = true;
            }
        }
    }
    QrCode::new(&dark, SIDE)
}
