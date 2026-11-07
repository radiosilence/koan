//! The storybook: every component and container of koan-kit on a page of
//! its own, in each of its states, over the wash. Along the top, what the
//! theme takes from the app and the system: the appearance, the accent (a
//! sample sleeve among them, to show its tone mapping and the wash), the
//! text size, Reduce Motion and Show icons. `cargo run -p koan-kit` opens
//! it; `examples/storybook.rs` draws each page offscreen instead.
//!
//! The window's root is the shape every app window takes: the wash and the
//! content as two cached views, so a frame of the drift redraws the wash
//! alone.

mod charts;
mod more;
mod playground;

use std::sync::Arc;

use crate::components::KoanFrosted;
use crate::components::field::FieldState as InputState;
use gpui::prelude::*;
use gpui::*;

use crate::components::{
    Availability, AvailabilityMark, Badge, Banner, Button, ButtonKind, Chip, Clear, CopyButton,
    Cover, CoverArt, DragChip, EmptyState, FormRow, INSERTION, IconToggle, KoanContextMenu,
    KoanInsets, Level, ListRow, Menu, MenuEntry, MenuPanel, MenuRow, NavRow, PageHeader, Picker,
    Placeholder, ProgressBar, ProgressRing, ProgressRow, RowKind, ScrollArea, SectionHeader,
    SectionHeading, Segmented, SelectionBar, Slider, Sparkline, Spinner, StatusBlock, StatusLine,
    Stepper, TabStrip, TableRow, TextArea, TextField, TileArt, TileMark, TileSleeve, Toggle,
    Toolbar, ToolbarButton, ToolbarTitle, Tooltip,
};
use crate::theme::{
    self, Accent, HAIRLINE, Icon, KoanIcon, KoanStyled, KoanTheme, Look, Space, Tone, TypeRole,
    wash::{Sleeve, Wash},
};

/// A page of the storybook.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Story {
    Type,
    Buttons,
    Icons,
    Controls,
    Forms,
    Text,
    Rows,
    Menus,
    Badges,
    Covers,
    Progress,
    Feedback,
    Bars,
    Collections,
    Lists,
    Panels,
    Motion,
    Guide,
    Rainbow,
    Charts,
    Playground,
}

impl Story {
    pub const ALL: [Story; 21] = [
        Story::Type,
        Story::Buttons,
        Story::Icons,
        Story::Controls,
        Story::Forms,
        Story::Text,
        Story::Rows,
        Story::Menus,
        Story::Badges,
        Story::Covers,
        Story::Progress,
        Story::Feedback,
        Story::Bars,
        Story::Collections,
        Story::Lists,
        Story::Panels,
        Story::Motion,
        Story::Guide,
        Story::Rainbow,
        Story::Charts,
        Story::Playground,
    ];

    /// Its name in the sidebar, and its PNG's.
    pub fn name(self) -> &'static str {
        match self {
            Story::Type => "type and colour",
            Story::Buttons => "buttons",
            Story::Icons => "icons",
            Story::Controls => "controls",
            Story::Forms => "fields and forms",
            Story::Text => "text fields",
            Story::Rows => "rows",
            Story::Menus => "menus and tooltips",
            Story::Badges => "badges and chips",
            Story::Covers => "covers and tiles",
            Story::Progress => "progress",
            Story::Feedback => "feedback",
            Story::Bars => "toolbars and headers",
            Story::Collections => "collections",
            Story::Lists => "tables and lists",
            Story::Panels => "panels and sheets",
            Story::Motion => "motion and drag",
            Story::Guide => "choices and checklists",
            Story::Rainbow => "rainbow",
            Story::Charts => "charts",
            Story::Playground => "motion playground",
        }
    }

    /// Its file name, without the extension.
    pub fn slug(self) -> String {
        self.name().replace(' ', "-")
    }
}

/// The accents the controls offer: none, two records' colours, and the
/// sample sleeve, which also gives the wash.
const ACCENTS: [&str; 4] = ["mint", "warm", "navy", "sleeve"];

/// The window's root: the wash beneath, the storybook over it.
pub struct Storybook {
    wash: Entity<Wash>,
    content: Entity<StoryContent>,
    _appearance: Subscription,
}

impl Storybook {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wash = cx.new(|_| Wash::new());
        let content = cx.new(|cx| StoryContent::new(wash.clone(), window, cx));
        Self {
            wash,
            content,
            _appearance: theme::follow_appearance(window, cx),
        }
    }

    /// Shows `story`: what the offscreen mode turns the pages with.
    pub fn show(&self, story: Story, cx: &mut App) {
        self.content.update(cx, |content, cx| {
            content.story = story;
            cx.notify();
        });
    }

    /// Takes the sample sleeve's accent and wash, as choosing "sleeve" does.
    pub fn use_sleeve(&self, cx: &mut App) {
        self.content
            .update(cx, |content, cx| content.choose_accent(3, cx));
    }
}

impl Render for Storybook {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                div().absolute().size_full().child(
                    self.wash
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .clear_of_system(Clear::Keyboard, window)
                    .child(
                        self.content
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
    }
}

/// The storybook's own state: the page and the controls' values, and what
/// the components on the pages hold.
pub struct StoryContent {
    wash: Entity<Wash>,
    story: Story,
    appearance: usize,
    accent: usize,
    gapless: bool,
    grouped: bool,
    segment: usize,
    picked: Option<usize>,
    level: f32,
    volume: u32,
    buffer: i64,
    playing: bool,
    selected_row: usize,
    tab: usize,
    field: Entity<InputState>,
    empty_field: Entity<InputState>,
    /// The bare field's own, so typing in it changes no other field.
    bare_field: Entity<InputState>,
    /// The text fields page's own: a long line, a password, a disabled
    /// field and several lines.
    long_field: Entity<InputState>,
    secret: Entity<InputState>,
    still: Entity<InputState>,
    fixed: Entity<InputState>,
    area: Entity<InputState>,
    tooltip: Entity<Tooltip>,
    outputs: Entity<Menu>,
    more: more::State,
    playground: playground::State,
    charts: Entity<charts::Charts>,
    /// On a narrow window, whether the pages and the controls are open over
    /// the page, from the menu button.
    menu_open: bool,
    /// The list rows' order, as dragging leaves it: indices into `ROWS`.
    rows: Vec<usize>,
    _accent: Subscription,
}

impl StoryContent {
    fn new(wash: Entity<Wash>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| InputState::new(window, cx).default_value("Sunday morning"));
        let empty_field = cx.new(|cx| InputState::new(window, cx).placeholder("search"));
        let bare_field = cx.new(|cx| InputState::new(window, cx).placeholder("a title"));
        let long_field = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(
                    "%album artist%/%date% - %album%/%disc%%tracknumber% %title% (%codec%, %bitrate%)",
                )
                // A code field: its owner's highlighter, the `%field%`s in
                // the accent.
                .highlight(|text| {
                    let marks: Vec<usize> = text.match_indices('%').map(|(i, _)| i).collect();
                    marks
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|[open, close]| (*open..close + 1, Tone::Accent))
                        .collect()
                })
        });
        let fixed = cx.new(|cx| {
            InputState::new(window, cx)
                .read_only(true)
                .default_value("https://koan.blit.cc/invite/7Q4M")
        });
        let secret = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .default_value("correct horse battery staple")
        });
        let still = cx.new(|cx| InputState::new(window, cx).default_value("signing in…"));
        let area = cx.new(|cx| {
            InputState::new(window, cx)
                .rows(4)
                .default_value("20 -1.2\n100 0.4\n1000 0.0\n10000 -2.1\n20000 -6.0")
        });
        Self {
            wash,
            story: Story::Type,
            appearance: 0,
            accent: 0,
            gapless: false,
            grouped: true,
            segment: 0,
            picked: Some(1),
            level: 0.38,
            volume: 40,
            buffer: 4,
            playing: false,
            selected_row: 1,
            tab: 0,
            field,
            empty_field,
            bare_field,
            long_field,
            secret,
            still,
            fixed,
            area,
            tooltip: cx.new(|_| Tooltip::new("add to queue")),
            outputs: Menu::build(window, cx, |menu, _, _| {
                menu.section("output")
                    .item(
                        MenuEntry::new("system default")
                            .icon(Icon::Output)
                            .rich("whatever the system plays through")
                            .checked(false)
                            .on_click(|_, _, _| {}),
                    )
                    .item(
                        MenuEntry::data("Living room")
                            .icon(Icon::Speaker)
                            .reach(Icon::Wifi)
                            .rich("WiiM Pro · upnp · flat")
                            .checked(true)
                            .on_click(|_, _, _| {}),
                    )
                    .control(|_, _| {
                        Level::new("story-volume", 40, 100)
                            .numbered()
                            .into_any_element()
                    })
                    .note("Living room plays the original files. ReplayGain and fades don't apply.")
            }),
            more: more::State::new(window, cx),
            playground: playground::State::new(),
            charts: cx.new(|_| charts::Charts::new()),
            menu_open: false,
            rows: (0..ROWS.len()).collect(),
            _accent: theme::observe_accent(cx),
        }
    }

    /// Takes accent `index` of `ACCENTS`; the sleeve also gives the wash,
    /// worked out off the main thread.
    fn choose_accent(&mut self, index: usize, cx: &mut Context<Self>) {
        self.accent = index;
        let fixed = match index {
            1 => Some(Accent::of(Some([0.94, 0.54, 0.36]))),
            2 => Some(Accent::of(Some([0.04, 0.10, 0.23]))),
            3 => None,
            _ => Some(Accent::MINT),
        };
        let wash = self.wash.clone();
        match fixed {
            Some(accent) => {
                KoanTheme::set_accent(accent, true, cx);
                wash.update(cx, |wash, cx| wash.set_sleeve(None, cx));
            }
            None => {
                cx.spawn(async move |_, cx| {
                    let (sleeve, accent) = cx
                        .background_executor()
                        .spawn(async {
                            let pixels = sleeve_pixels();
                            let accent = Accent::of_sleeve(&pixels, SAMPLE, SAMPLE);
                            (Sleeve::new(Arc::from(pixels), SAMPLE, SAMPLE), accent)
                        })
                        .await;
                    cx.update(|cx| {
                        KoanTheme::set_accent(accent, true, cx);
                        wash.update(cx, |wash, cx| wash.set_sleeve(sleeve, cx));
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// The controls along the top.
    fn controls(&self, look: &Look, cx: &mut Context<Self>) -> Div {
        let entity = cx.entity();
        let text_size = cx.global::<KoanTheme>().text_size;
        let reduce = cx.reduce_motion();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.space(Space::L))
            .px(look.space(Space::L))
            .py(look.space(Space::S))
            .border_b(HAIRLINE)
            .border_color(look.palette.rule.hsla())
            .child(
                Segmented::new("appearance", ["system", "light", "dark"], self.appearance)
                    .on_change({
                        let entity = entity.clone();
                        move |index, window, cx| {
                            let index = *index;
                            theme::pin_appearance(
                                match index {
                                    1 => Some(false),
                                    2 => Some(true),
                                    _ => None,
                                },
                                window,
                                cx,
                            );
                            entity.update(cx, |story, cx| {
                                story.appearance = index;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(Segmented::new("accent", ACCENTS, self.accent).on_change({
                let entity = entity.clone();
                move |index, _, cx| {
                    let index = *index;
                    entity.update(cx, |story, cx| story.choose_accent(index, cx));
                }
            }))
            .child(
                Stepper::new("text-size", "text size", i64::from(text_size), 80..=130)
                    .step(i64::from(theme::tokens::TEXT_SIZE_STEP))
                    .shown(format!("{text_size}%"))
                    .on_change(|size, _, cx| KoanTheme::set_text_size(*size as u32, cx)),
            )
            .child(
                Toggle::new("reduce-motion", "reduce motion", reduce).on_change(|on, _, cx| {
                    cx.set_reduce_motion(*on);
                    cx.refresh_windows();
                }),
            )
            .child(
                Toggle::new("show-icons", "show icons", look.icons)
                    .on_change(|on, _, cx| KoanTheme::set_icons(*on, cx)),
            )
            .child(
                Toggle::new("touch", "touch", look.touch)
                    .on_change(|on, _, cx| KoanTheme::set_touch(*on, cx)),
            )
    }

    fn page(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let entity = cx.entity();
        let set = |change: fn(&mut StoryContent, bool)| {
            let entity = entity.clone();
            move |value: &bool, _: &mut Window, cx: &mut App| {
                let value = *value;
                entity.update(cx, |story, cx| {
                    change(story, value);
                    cx.notify();
                })
            }
        };
        let row = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.space(Space::M))
        };
        match self.story {
            Story::Playground => self.playground(look, window, cx),
            Story::Charts => vec![self.charts.clone().into_any_element()],
            Story::Collections
            | Story::Lists
            | Story::Panels
            | Story::Motion
            | Story::Guide
            | Story::Rainbow => {
                self.more(look, window, cx)
            }
            Story::Type => {
                let mut v = vec![SectionHeader::new("Type").into_any_element()];
                v.extend([
                    text("kōan", TypeRole::Display, Tone::Strong, look),
                    text("albums", TypeRole::Title, Tone::Strong, look),
                    text("Low Tide Arcade", TypeRole::TitleSmall, Tone::Strong, look),
                    text("Body text, in ink.", TypeRole::Body, Tone::Ink, look),
                    text("A control's label.", TypeRole::Control, Tone::Ink, look),
                    text("Secondary text, muted.", TypeRole::Meta, Tone::Muted, look),
                    text(
                        "The record's accent, as text.",
                        TypeRole::Body,
                        Tone::Accent,
                        look,
                    ),
                    text("Something went wrong.", TypeRole::Fine, Tone::Bad, look),
                ]);
                v.push(SectionHeader::new("Palette").into_any_element());
                let pal = &look.palette;
                let swatches = [
                    ("bg", pal.bg.hsla()),
                    ("surface", pal.surface.hsla()),
                    ("hover", pal.hover.hsla()),
                    ("rule", pal.rule.hsla()),
                    ("muted", pal.muted.hsla()),
                    ("ink", pal.ink.hsla()),
                    ("strong", pal.strong.hsla()),
                    ("bad", pal.bad.hsla()),
                    ("selection", pal.selection.hsla()),
                    ("accent", look.accent.hsla()),
                ];
                v.push(
                    row()
                        .children(swatches.into_iter().map(|(name, colour)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap(look.space(Space::Xs))
                                .child(
                                    div()
                                        .size(look.spaced(48.0))
                                        .bg(colour)
                                        .border(HAIRLINE)
                                        .border_color(pal.rule.hsla()),
                                )
                                .child(text(name, TypeRole::Fine, Tone::Muted, look))
                        }))
                        .into_any_element(),
                );
                v
            }
            Story::Buttons => {
                let playing = self.playing;
                let wash = self.wash.clone();
                let play_entity = entity.clone();
                vec![
                    SectionHeader::new("Kinds").into_any_element(),
                    row()
                        .child(
                            Button::new("play", ButtonKind::Prominent)
                                .label("Play")
                                .icon(Icon::Play)
                                .on_click(move |_, _, cx| {
                                    let playing = !playing;
                                    play_entity.update(cx, |story, cx| {
                                        story.playing = playing;
                                        cx.notify();
                                    });
                                    wash.update(cx, |wash, cx| wash.drift(playing, cx));
                                }),
                        )
                        .child(
                            Button::new("shuffle", ButtonKind::Standard)
                                .label("Shuffle")
                                .icon(Icon::Shuffle),
                        )
                        .child(
                            Button::new("favourite", ButtonKind::Compact)
                                .label("Favourite")
                                .icon(Icon::Favourite),
                        )
                        .child(CopyButton::new(
                            "copy",
                            "copy link",
                            "https://koan.blit.cc/invite/7Q4M",
                        ))
                        .child(Button::new("token", ButtonKind::Link).label("Find your token"))
                        .child(Button::new("clear", ButtonKind::Text).label("Clear"))
                        .into_any_element(),
                    row()
                        .child(
                            Button::new("previous", ButtonKind::Icon)
                                .label("Previous")
                                .icon(Icon::Previous),
                        )
                        .child(
                            Button::new("pause", ButtonKind::IconOutlined)
                                .label(if playing { "Pause" } else { "Play" })
                                .icon(if playing { Icon::Pause } else { Icon::Play }),
                        )
                        .child(
                            Button::new("next", ButtonKind::Icon)
                                .label("Next")
                                .icon(Icon::Next),
                        )
                        .child(
                            Button::new("sidebar", ButtonKind::Toolbar)
                                .label("Sidebar")
                                .icon(Icon::Sidebar),
                        )
                        .child(
                            Button::new("lyrics", ButtonKind::Toolbar)
                                .label("Lyrics")
                                .icon(Icon::Lyrics)
                                .selected(true),
                        )
                        .into_any_element(),
                    SectionHeader::new("States").into_any_element(),
                    row()
                        .child(
                            Button::new("remove", ButtonKind::Compact)
                                .label("Remove")
                                .destructive(true),
                        )
                        .child(
                            Button::new("forget", ButtonKind::Bordered)
                                .label("Forget")
                                .destructive(true),
                        )
                        .child(
                            Button::new("disabled", ButtonKind::Standard)
                                .label("Disabled")
                                .disabled(true),
                        )
                        .child(
                            Button::new("disabled-prominent", ButtonKind::Prominent)
                                .label("Sign in")
                                .disabled(true),
                        )
                        .child(
                            Button::new("helped", ButtonKind::Icon)
                                .label("Back")
                                .icon(Icon::Back)
                                .help("back (⌘[)"),
                        )
                        .into_any_element(),
                ]
            }
            Story::Icons => vec![
                SectionHeader::new("The set").into_any_element(),
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.space(Space::S))
                    .children(Icon::ALL.iter().map(|icon| {
                        KoanIcon::new(*icon)
                            .role(TypeRole::Control)
                            .into_any_element()
                    }))
                    .into_any_element(),
                SectionHeader::new("Fills and sizes").into_any_element(),
                row()
                    .child(
                        KoanIcon::new(Icon::Favourited)
                            .role(TypeRole::Title)
                            .tone(Tone::Accent),
                    )
                    .child(
                        KoanIcon::new(Icon::Selecting)
                            .role(TypeRole::Title)
                            .tone(Tone::Ink),
                    )
                    .child(
                        KoanIcon::new(Icon::Selecting)
                            .role(TypeRole::Title)
                            .tone(Tone::Strong)
                            .fill(look.accent.hsla()),
                    )
                    .child(KoanIcon::new(Icon::Play).role(TypeRole::Fine))
                    .child(KoanIcon::new(Icon::Play).role(TypeRole::Body))
                    .child(KoanIcon::new(Icon::Play).role(TypeRole::Display))
                    .into_any_element(),
            ],
            Story::Controls => vec![
                SectionHeader::new("Toggles").into_any_element(),
                Toggle::new("gapless", "Gapless", self.gapless)
                    .on_change(set(|s, v| s.gapless = v))
                    .into_any_element(),
                Toggle::new("locked", "Locked", true)
                    .disabled(true)
                    .into_any_element(),
                row()
                    .child(
                        IconToggle::new("grouped", Icon::Album, "Group by record", self.grouped)
                            .on_change(set(|s, v| s.grouped = v)),
                    )
                    .child(IconToggle::new(
                        "flat",
                        Icon::QueueSection,
                        "Flat",
                        !self.grouped,
                    ))
                    .into_any_element(),
                SectionHeader::new("Choices").into_any_element(),
                Segmented::new("filter", ["All", "Favourites", "Lossless"], self.segment)
                    .on_change({
                        let entity = entity.clone();
                        move |index, _, cx| {
                            let index = *index;
                            entity.update(cx, |story, cx| {
                                story.segment = index;
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
                Picker::new("sort", ["Title", "Artist", "Year added"], self.picked)
                    .on_change({
                        let entity = entity.clone();
                        move |index, _, cx| {
                            let index = *index;
                            entity.update(cx, |story, cx| {
                                story.picked = Some(index);
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
                SectionHeader::new("Values").into_any_element(),
                Slider::new("level", self.level)
                    .ends("1:24", "-2:17")
                    .on_change({
                        let entity = entity.clone();
                        move |level, _, cx| {
                            let level = *level;
                            entity.update(cx, |story, cx| {
                                story.level = level;
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
                Level::new("volume", self.volume, 100)
                    .numbered()
                    .on_commit({
                        let entity = entity.clone();
                        move |volume, _, cx| {
                            let volume = *volume;
                            entity.update(cx, |story, cx| {
                                story.volume = volume;
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
                Stepper::new("buffer", "Buffer", self.buffer, 1..=8)
                    .shown(format!("{} s", self.buffer))
                    .on_change({
                        let entity = entity.clone();
                        move |buffer, _, cx| {
                            let buffer = *buffer;
                            entity.update(cx, |story, cx| {
                                story.buffer = buffer;
                                cx.notify();
                            })
                        }
                    })
                    .into_any_element(),
            ],
            Story::Forms => vec![
                SectionHeader::new("Playback").into_any_element(),
                FormRow::new("Output")
                    .value("Built-in Output")
                    .into_any_element(),
                FormRow::new("Name")
                    .control(
                        div()
                            .w(look.spaced(220.0))
                            .child(TextField::new(&self.field)),
                    )
                    .into_any_element(),
                FormRow::data("Oratory1990")
                    .value("harman")
                    .into_any_element(),
                FormRow::new("Server")
                    .control(Button::new("sign-out", ButtonKind::Bordered).label("Sign out"))
                    .into_any_element(),
                SectionHeader::new("In a popover").into_any_element(),
                div()
                    .w(look.spaced(300.0))
                    .child(FormRow::new("Genre").compact().value("Any"))
                    .child(FormRow::new("Years").compact().value("1990 – 1999"))
                    .into_any_element(),
                SectionHeader::new("Bare on a surface").into_any_element(),
                div()
                    .p(look.space(Space::M))
                    .bg(look.palette.surface.hsla())
                    .child(TextField::new(&self.empty_field).bare())
                    .into_any_element(),
            ],
            Story::Text => {
                let field = |state: &Entity<InputState>| {
                    div().w(look.spaced(280.0)).child(TextField::new(state))
                };
                vec![
                    SectionHeader::new("A line").into_any_element(),
                    field(&self.field).into_any_element(),
                    SectionHeader::new("Empty, with its placeholder").into_any_element(),
                    field(&self.empty_field).into_any_element(),
                    SectionHeader::new(
                        "Longer than the field, its fields highlighted: it scrolls to the caret",
                    )
                    .into_any_element(),
                    field(&self.long_field).into_any_element(),
                    SectionHeader::new("A password").into_any_element(),
                    field(&self.secret).into_any_element(),
                    SectionHeader::new("Read-only: selected and copied, never changed")
                        .into_any_element(),
                    field(&self.fixed).into_any_element(),
                    SectionHeader::new("Disabled").into_any_element(),
                    div()
                        .w(look.spaced(280.0))
                        .child(TextField::new(&self.still).disabled(true))
                        .into_any_element(),
                    SectionHeader::new("Bare, on the page's ground").into_any_element(),
                    div()
                        .w(look.spaced(280.0))
                        .child(
                            TextField::new(&self.bare_field)
                                .bare()
                                .role(TypeRole::Title),
                        )
                        .into_any_element(),
                    SectionHeader::new("Several lines").into_any_element(),
                    div()
                        .w(look.spaced(280.0))
                        .child(TextArea::new(&self.area))
                        .into_any_element(),
                ]
            }
            Story::Rows => {
                let selected_row = self.selected_row;
                let accent = look.accent.hsla();
                vec![
                    SectionHeader::new("List rows").into_any_element(),
                    div()
                        .flex()
                        .flex_col()
                        .children(
                            self.rows.iter().enumerate().map(|(at, &track)| {
                                let (title, length) = ROWS[track];
                                let entity = entity.clone();
                                let dropped = entity.clone();
                                // Held, a row opens its menu; held and moved,
                                // it lifts and drops between the others.
                                div()
                                    .id(("row-drag", track))
                                    .on_drag(RowDrag(track), |row, _, _, cx| {
                                        let title = ROWS[row.0].0;
                                        cx.new(|_| RowChip(title))
                                    })
                                    .drag_over::<RowDrag>(move |style, _, _, _| {
                                        style.border_t(INSERTION).border_color(accent)
                                    })
                                    .on_drop(move |row: &RowDrag, _, cx| {
                                        dropped.update(cx, |story, cx| {
                                            story.rows.retain(|&t| t != row.0);
                                            let at = at.min(story.rows.len());
                                            story.rows.insert(at, row.0);
                                            cx.notify();
                                        })
                                    })
                                    .child(
                                        ListRow::new(("row", track))
                                            .number((at + 1).to_string())
                                            .title(title)
                                            .detail("Low Tide Arcade")
                                            .figure(length)
                                            .selected(track == selected_row)
                                            .playing(track == 2)
                                            .on_click(move |_, _, cx| {
                                                entity.update(cx, |story, cx| {
                                                    story.selected_row = track;
                                                    cx.notify();
                                                })
                                            }),
                                    )
                                    .koan_context_menu(
                                        ("row-menu", track),
                                        window,
                                        cx,
                                        |menu, _, _| {
                                            menu.item(
                                                MenuEntry::new("play next")
                                                    .icon(Icon::Play)
                                                    .on_click(|_, _, _| {}),
                                            )
                                            .item(
                                                MenuEntry::new("remove")
                                                    .destructive()
                                                    .on_click(|_, _, _| {}),
                                            )
                                        },
                                    )
                            }),
                        )
                        .into_any_element(),
                    SectionHeader::new("Table rows").into_any_element(),
                    div()
                        .flex()
                        .flex_col()
                        .child(TableRow::new("table-line", RowKind::Line).title(text(
                            "A row of one line",
                            TypeRole::Body,
                            Tone::Ink,
                            look,
                        )))
                        .child(
                            TableRow::new("table-text", RowKind::Text)
                                .title(text("Neon Breakwater", TypeRole::Body, Tone::Ink, look))
                                .line(text("Low Tide Arcade", TypeRole::Meta, Tone::Muted, look))
                                .figure(48.0, text("5:17", TypeRole::Meta, Tone::Muted, look)),
                        )
                        .child(
                            TableRow::new("table-selected", RowKind::Text)
                                .selected(true)
                                .title(text("A picked row", TypeRole::Body, Tone::Ink, look))
                                .line(text("its second line", TypeRole::Meta, Tone::Muted, look)),
                        )
                        .into_any_element(),
                    SectionHeader::new("Navigation").into_any_element(),
                    div()
                        .flex()
                        .flex_col()
                        .w(look.spaced(220.0))
                        .child(
                            NavRow::new("nav-queue", "Queue")
                                .icon(Icon::QueueSection)
                                .selected(true),
                        )
                        .child(NavRow::new("nav-albums", "Albums").icon(Icon::Album))
                        .child(NavRow::new("nav-artists", "Artists").icon(Icon::Artist))
                        .child(NavRow::new("nav-history", "History").icon(Icon::History))
                        .into_any_element(),
                ]
            }
            Story::Menus => vec![
                SectionHeader::new("A menu's rows").into_any_element(),
                div()
                    .w(look.spaced(240.0))
                    .child(
                        MenuPanel::new()
                            .child(MenuRow::new("sleep-off", "Off").checked(true))
                            .child(
                                MenuRow::new("sleep-15", "In 15 minutes")
                                    .checked(false)
                                    .highlighted(true),
                            )
                            .child(
                                MenuRow::new("sleep-record", "At the end of the record")
                                    .checked(false),
                            )
                            .child(MenuRow::new("opens", "Add to playlist").opens(true))
                            .child(
                                MenuRow::new("forget", "Forget")
                                    .icon(Icon::Clear)
                                    .destructive(true),
                            )
                            .child(MenuRow::new("off", "Unavailable").disabled(true)),
                    )
                    .into_any_element(),
                SectionHeader::new("Rich rows, a control and a note").into_any_element(),
                div()
                    .w(look.spaced(340.0))
                    .child(self.outputs.clone())
                    .into_any_element(),
                SectionHeader::new("Tooltip").into_any_element(),
                div().child(self.tooltip.clone()).into_any_element(),
                SectionHeader::new(
                    "Opening a panel: pressed while it shows; a press outside, Escape or the trigger closes it",
                )
                .into_any_element(),
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::L))
                    .child(crate::components::ToolbarMenu::new(
                        "story-panel-menu",
                        crate::components::ToolbarButton::new("story-panel-sort", Icon::Sort, "sort"),
                        |dismiss, _, _| {
                            MenuPanel::new()
                                .child(MenuRow::new("story-sort-title", "Title").checked(true))
                                .child(
                                    MenuRow::new("story-sort-year", "Year")
                                        .checked(false)
                                        .on_click(move |_, window, cx| dismiss(window, cx)),
                                )
                        },
                    ))
                    .child(crate::components::opens_panel(
                        "story-panel-word",
                        crate::components::TextTrigger::new("story-panel-trigger", "any genre"),
                        |_, _, _| {
                            MenuPanel::new()
                                .child(MenuRow::new("story-genre-any", "Any").checked(true))
                                .child(MenuRow::new("story-genre-jazz", "Jazz").checked(false))
                                .into_any_element()
                        },
                    ))
                    .into_any_element(),
            ],
            Story::Badges => vec![
                SectionHeader::new("Badges").into_any_element(),
                row()
                    .child(Badge::new("FLAC 24/96"))
                    .child(Badge::new("Bit-perfect").accent(true))
                    .into_any_element(),
                SectionHeader::new("Chips").into_any_element(),
                row()
                    .child(Chip::new("chip-dac", "Topping E30"))
                    .child(Chip::new("chip-preset", "Harman 2018"))
                    .into_any_element(),
                SectionHeader::new("Availability").into_any_element(),
                row()
                    .child(AvailabilityMark::new(Some(Availability::Pending)))
                    .child(AvailabilityMark::new(Some(Availability::Queued)))
                    .child(AvailabilityMark::new(Some(Availability::Failed)))
                    .child(AvailabilityMark::new(Some(Availability::NotHere)))
                    .child(AvailabilityMark::new(Some(Availability::Stored {
                        on_server: true,
                        on_disk: true,
                    })))
                    .child(AvailabilityMark::new(Some(Availability::Stored {
                        on_server: true,
                        on_disk: false,
                    })))
                    .child(AvailabilityMark::new(Some(Availability::Stored {
                        on_server: false,
                        on_disk: true,
                    })))
                    .into_any_element(),
            ],
            Story::Covers => {
                let side = look.spaced(120.0);
                let warm = gpui::hsla(0.05, 0.6, 0.45, 1.0);
                vec![
                    SectionHeader::new("Covers").into_any_element(),
                    row()
                        .child(Cover::new(CoverArt::Colour(warm), side))
                        .child(Cover::new(CoverArt::Loading, side))
                        .child(Cover::new(CoverArt::Missing, side))
                        .into_any_element(),
                    SectionHeader::new("Placeholder and spinner").into_any_element(),
                    row()
                        .child(Placeholder::new(look.spaced(64.0)))
                        .child(Placeholder::new(look.spaced(64.0)).loading(true))
                        .child(Spinner::new(look.spaced(24.0)))
                        .child(Spinner::new(look.spaced(24.0)).glow())
                        .child(Spinner::new(look.font_size(TypeRole::Fine)).glow())
                        .into_any_element(),
                    SectionHeader::new("Record tiles").into_any_element(),
                    row()
                        .child(
                            TileArt::new(("story-tile", 1usize), side, TileSleeve::Colour(warm))
                                .codec(Some("FLAC".into()))
                                .badge(Some("Deluxe".into()))
                                .heart(true, false, true),
                        )
                        .child(
                            TileArt::new(("story-tile", 2usize), side, TileSleeve::Loading)
                                .mark(Some(TileMark::Play)),
                        )
                        .child(
                            TileArt::new(("story-tile", 3usize), side, TileSleeve::Missing)
                                .on_device(Some(0.4))
                                .mark(Some(TileMark::Starting)),
                        )
                        .child(
                            TileArt::new(("story-tile", 4usize), side, TileSleeve::Colour(warm))
                                .pick(Some(true)),
                        )
                        .child(
                            TileArt::new(("story-tile", 5usize), side, TileSleeve::Colour(warm))
                                .pick(Some(false)),
                        )
                        .into_any_element(),
                ]
            }
            Story::Progress => vec![
                SectionHeader::new("Bars").into_any_element(),
                div()
                    .w(look.spaced(320.0))
                    .child(ProgressBar::new("measured", Some(0.62)))
                    .into_any_element(),
                div()
                    .w(look.spaced(320.0))
                    .child(ProgressBar::new("unmeasured", None))
                    .into_any_element(),
                SectionHeader::new("Rings").into_any_element(),
                row()
                    .child(ProgressRing::new(Some(0.25), look.palette.muted.hsla()))
                    .child(ProgressRing::new(Some(0.75), look.palette.muted.hsla()))
                    .child(ProgressRing::new(None, look.palette.muted.hsla()))
                    .into_any_element(),
                SectionHeader::new("A sidebar's foot").into_any_element(),
                div()
                    .w(look.spaced(240.0))
                    .child(
                        ProgressRow::new("scan", "Scanning")
                            .counts("1,204 of 5,120")
                            .detail("Low Tide Arcade")
                            .fraction(Some(0.24))
                            .on_stop(|_, _, _| {}),
                    )
                    .child(
                        StatusBlock::new()
                            .line(Icon::Server, "connected to koan.blit.cc", Tone::Muted)
                            .figure("5,123 tracks"),
                    )
                    .into_any_element(),
                SectionHeader::new("A curve").into_any_element(),
                Sparkline::new(
                    (0..64).map(|i| (i as f64 / 6.0).sin() * 4.0).collect(),
                    6.0,
                    look.accent.hsla(),
                )
                .into_any_element(),
            ],
            Story::Feedback => vec![
                SectionHeader::new("Banner").into_any_element(),
                Banner::new(Icon::Warning, "Signed out of your server", Tone::Bad)
                    .action("sign in", |_, _, _| {})
                    .into_any_element(),
                Banner::new(
                    Icon::Offline,
                    "Can't reach your server. kōan keeps trying.",
                    Tone::Bad,
                )
                .into_any_element(),
                Banner::new(Icon::Speaker, "Waiting for USB DAC…", Tone::Muted)
                    .action("retry", |_, _, _| {})
                    .action("cancel", |_, _, _| {})
                    .into_any_element(),
                SectionHeader::new("Status line").into_any_element(),
                StatusLine::new("Signed in as james.", Tone::Muted, Some(Icon::Success))
                    .into_any_element(),
                StatusLine::new(
                    "Could not reach the server.",
                    Tone::Bad,
                    Some(Icon::WarningFilled),
                )
                .into_any_element(),
                SectionHeader::new("Empty").into_any_element(),
                div()
                    .h(look.spaced(200.0))
                    .border(HAIRLINE)
                    .border_color(look.palette.rule.hsla())
                    .child(
                        EmptyState::new(Icon::Playlist, "No playlists")
                            .detail("Make one from the queue, or drop tracks here."),
                    )
                    .into_any_element(),
                SectionHeader::new("Empty, in small areas: drawn smaller, then clipped")
                    .into_any_element(),
                div()
                    .flex()
                    .gap(look.space(Space::M))
                    .children([(220.0, 160.0), (150.0, 200.0), (90.0, 90.0)].map(
                        |(width, height)| {
                            let id = format!("story-empty-{width}");
                            div()
                                .w(look.spaced(width))
                                .h(look.spaced(height))
                                .border(HAIRLINE)
                                .border_color(look.palette.rule.hsla())
                                .child(
                                    EmptyState::new(Icon::QueueSection, "queue is empty")
                                        .id(gpui::ElementId::Name(id.into()))
                                        .detail("Find something to play from albums or artists."),
                                )
                        },
                    ))
                    .into_any_element(),
                SectionHeader::new("Picking").into_any_element(),
                SelectionBar::new("3 picked")
                    .verb("story-play", "play", |_, _, _| {})
                    .verb("story-queue", "add to queue", |_, _, _| {})
                    .on_done(|_, _, _| {})
                    .into_any_element(),
            ],
            Story::Bars => {
                let tab = self.tab;
                let tabs = ["Library", "Server", "Playback"]
                    .into_iter()
                    .enumerate()
                    .fold(TabStrip::new(), |strip, (index, title)| {
                        let entity = entity.clone();
                        let icon = [Icon::Album, Icon::Server, Icon::Speaker][index];
                        strip.tab(icon, title, index == tab, move |_, _, cx| {
                            entity.update(cx, |story, cx| {
                                story.tab = index;
                                cx.notify();
                            })
                        })
                    });
                vec![
                    SectionHeader::new("Toolbar").into_any_element(),
                    Toolbar::new()
                        .leading(
                            ToolbarButton::new("tb-sidebar", Icon::Sidebar, "sidebar")
                                .help("show or hide the sidebar"),
                        )
                        .leading(ToolbarButton::new("tb-back", Icon::Back, "back"))
                        .principal(
                            div()
                                .flex()
                                .items_center()
                                .child(ToolbarTitle::new("queue")),
                        )
                        .trailing(ToolbarButton::new("tb-queue", Icon::Queue, "queue"))
                        .trailing(
                            ToolbarButton::new("tb-lyrics", Icon::Lyrics, "lyrics").selected(true),
                        )
                        .into_any_element(),
                    SectionHeader::new("Tabs").into_any_element(),
                    tabs.into_any_element(),
                    SectionHeader::new("Page header").into_any_element(),
                    PageHeader::new("albums")
                        .detail(text("5,123 albums", TypeRole::Control, Tone::Muted, look))
                        .badge(Badge::new("smart"))
                        .into_any_element(),
                    SectionHeading::new("Appears on").into_any_element(),
                    SectionHeader::new("Frosted bar").into_any_element(),
                    frosted_bar(look),
                ]
            }
        }
    }
}

impl Render for StoryContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let entity = cx.entity();
        let story = self.story;
        // A phone's width: the pages and the controls behind a menu button,
        // so the page has the width to itself.
        let narrow = window.viewport_size().width < px(NARROW);
        let nav = div()
            .flex()
            .flex_col()
            .flex_none()
            .when(!narrow, |nav| {
                nav.w(look.spaced(220.0))
                    .border_r(HAIRLINE)
                    .border_color(look.palette.rule.hsla())
            })
            .pt(look.space(Space::L))
            .children(Story::ALL.into_iter().map(|page| {
                let entity = entity.clone();
                NavRow::new(page.slug(), page.name())
                    .selected(page == story)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |content, cx| {
                            content.story = page;
                            content.menu_open = false;
                            cx.notify();
                        })
                    })
            }));
        let controls = self.controls(&look, cx);
        let viewer = self.viewer(cx);
        let page = div().flex_1().min_h_0().child(ScrollArea::new(
            ("story", story as usize),
            div()
                .flex()
                .flex_col()
                .gap(look.space(Space::M))
                .p(look.space(if narrow { Space::L } else { Space::Xxl }))
                .children(self.page(&look, window, cx)),
        ));
        let root = div()
            .font_family(theme::FONT_FAMILY)
            .relative()
            .size_full()
            .flex();
        if !narrow {
            return root
                .child(nav)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(controls)
                        .child(page),
                )
                .children(viewer);
        }
        let open = self.menu_open;
        let toggle = entity.clone();
        root.flex_col()
            .child(
                Toolbar::new()
                    .leading(
                        ToolbarButton::new("story-menu", Icon::Sidebar, "pages")
                            .selected(open)
                            .on_click(move |_, _, cx| {
                                toggle.update(cx, |content, cx| {
                                    content.menu_open = !content.menu_open;
                                    cx.notify();
                                })
                            }),
                    )
                    .principal(ToolbarTitle::new(story.name())),
            )
            .child(if open {
                div().flex_1().min_h_0().child(ScrollArea::new(
                    "story-menu-list",
                    div().flex().flex_col().child(controls).child(nav),
                ))
            } else {
                page
            })
            .children(viewer)
    }
}

/// The rows page's tracks: a title and a length.
const ROWS: [(&str, &str); 5] = [
    ("Shoreline Pinball", "3:41"),
    ("Coin Return", "4:02"),
    ("Neon Breakwater", "5:17"),
    ("Tilt", "2:58"),
    ("High Score at Dusk", "6:30"),
];

/// A row of the rows page being dragged, by its index into `ROWS`.
struct RowDrag(usize);

/// What a dragged row shows under the finger.
struct RowChip(&'static str);

impl Render for RowChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        DragChip::new(self.0)
    }
}

/// The width below which the storybook folds its pages and controls behind
/// a menu button: a phone held upright.
const NARROW: f32 = 600.0;

fn text(content: &'static str, role: TypeRole, tone: Tone, look: &Look) -> AnyElement {
    div()
        .text_role(role, look)
        .text_color(look.text_color(tone))
        .child(content)
        .into_any_element()
}

/// The sample sleeve's side.
const SAMPLE: u32 = 96;

/// A sleeve for the accent and the wash: a warm field and a cool one over a
/// dark ground, so both the blur and the hue weighting show.
fn sleeve_pixels() -> Vec<u8> {
    let mut pixels = Vec::with_capacity((SAMPLE * SAMPLE * 4) as usize);
    for y in 0..SAMPLE {
        for x in 0..SAMPLE {
            let (fx, fy) = (x as f32 / SAMPLE as f32, y as f32 / SAMPLE as f32);
            let warm = (1.0 - ((fx - 0.3).powi(2) + (fy - 0.35).powi(2)).sqrt() * 2.6).max(0.0);
            let cool = (1.0 - ((fx - 0.75).powi(2) + (fy - 0.7).powi(2)).sqrt() * 3.2).max(0.0);
            let r = 20.0 + 225.0 * warm + 10.0 * cool;
            let g = 18.0 + 110.0 * warm + 150.0 * cool;
            let b = 30.0 + 30.0 * warm + 170.0 * cool;
            pixels.extend([r, g, b].map(|v| v.min(255.0) as u8));
            pixels.push(255);
        }
    }
    pixels
}

/// A phone's bar over a page that scrolls under it: colour fields scroll in
/// a short box, and the bar laid over its top blurs and tints them. Scroll
/// the box to see the frost; with less transparency asked for, the bar is
/// opaque.
fn frosted_bar(look: &Look) -> AnyElement {
    const BAR: f32 = 48.0;
    let fields = (0..12).map(|i| {
        let hue = i as f32 / 12.0;
        div()
            .h(px(64.0))
            .mx(px(16.0))
            .mb(px(8.0))
            .bg(linear_gradient(
                135.,
                linear_color_stop(hsla(hue, 0.75, 0.55, 1.0), 0.),
                linear_color_stop(hsla((hue + 0.2) % 1.0, 0.75, 0.45, 1.0), 1.),
            ))
            .child(
                div()
                    .p(px(8.0))
                    .text_color(hsla(0., 0., 1., 0.9))
                    .child(format!("Record {:02}", i + 1)),
            )
    });
    div()
        .relative()
        .w(px(360.0))
        .h(px(280.0))
        .overflow_hidden()
        .border(look.hairline)
        .border_color(look.palette.rule.hsla())
        .child(
            div()
                .id("frost-demo-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(div().h(px(BAR + 8.0)))
                .children(fields),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(BAR))
                .frosted(look)
                .child(
                    Toolbar::new()
                        .leading(ToolbarButton::new("frost-back", Icon::Back, "back"))
                        .principal(
                            div()
                                .flex()
                                .items_center()
                                .child(ToolbarTitle::new("albums")),
                        )
                        .trailing(ToolbarButton::new("frost-sort", Icon::Sort, "sort")),
                ),
        )
        .into_any_element()
}
