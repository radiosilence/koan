//! The theme sheet: every component of the kōan theme in every state, over
//! the wash, for checking a change to the theme before any screen. The Mac
//! app's `KoanThemeSheet`.
//!
//! It is also the shape a window's root takes: the wash and the content as
//! two cached views, so a frame of the drift redraws the wash alone.

use std::sync::Arc;

use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, Render, StyleRefinement,
    Styled, Subscription, Window, div, px,
};

use crate::components::{
    Badge, Button, ButtonKind, Chip, EmptyState, FormRow, IconToggle, Level, ListRow, Menu,
    MenuEntry, MenuPanel, MenuRow, NavRow, SectionHeader, Segmented, Slider, Stepper, TextField,
    Toggle, Tooltip,
};
use crate::theme::{
    self, Accent, Icon, KoanIcon, KoanStyled, KoanTheme, Look, Space, Tone, TypeRole,
    wash::{Sleeve, Wash},
};

/// The window's root: the wash beneath, the sheet over it.
pub struct ThemeSheet {
    wash: Entity<Wash>,
    content: Entity<SheetContent>,
    _appearance: Subscription,
}

impl ThemeSheet {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wash = cx.new(|_| Wash::new());
        let content = cx.new(|cx| SheetContent::new(wash.clone(), window, cx));
        // The sleeve, its accent and its wash are worked out off the main
        // thread; only the results are applied here.
        let sheet_wash = wash.clone();
        cx.spawn(async move |_, cx| {
            let (sleeve, accent) = cx
                .background_executor()
                .spawn(async {
                    let pixels = sleeve_pixels();
                    let accent = Accent::of_sleeve(&pixels, SAMPLE, SAMPLE);
                    let sleeve = Sleeve::new(Arc::from(pixels), SAMPLE, SAMPLE);
                    (sleeve, accent)
                })
                .await;
            cx.update(|cx| {
                KoanTheme::set_accent(accent, true, cx);
                sheet_wash.update(cx, |wash, cx| wash.set_sleeve(sleeve, cx));
            });
        })
        .detach();
        Self {
            wash,
            content,
            _appearance: theme::follow_appearance(window, cx),
        }
    }
}

impl Render for ThemeSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        KoanTheme::drive(window, cx);
        let fill = || {
            let mut style = StyleRefinement::default();
            style.size.width = Some(gpui_kit::relative(1.0).into());
            style.size.height = Some(gpui_kit::relative(1.0).into());
            style
        };
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(self.wash.clone().cached(fill())),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(self.content.clone().cached(fill())),
            )
    }
}

/// The sheet's own state: what the controls on it hold.
pub struct SheetContent {
    wash: Entity<Wash>,
    gapless: bool,
    grouped: bool,
    segment: usize,
    level: f32,
    buffer: i64,
    playing: bool,
    selected_row: usize,
    field: Entity<InputState>,
    empty_field: Entity<InputState>,
    /// A tooltip as it is drawn, held open for the sheet.
    tooltip: Entity<Tooltip>,
    /// An output menu as it is drawn: rich rows, a control and a note.
    outputs: Entity<Menu>,
    _accent: Subscription,
}

impl SheetContent {
    fn new(wash: Entity<Wash>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| InputState::new(window, cx).default_value("Sunday morning"));
        let empty_field = cx.new(|cx| InputState::new(window, cx).placeholder("search"));
        Self {
            wash,
            gapless: false,
            grouped: true,
            segment: 0,
            level: 0.38,
            buffer: 4,
            playing: false,
            selected_row: 1,
            field,
            empty_field,
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
                        Level::new("sheet-volume", 40, 100)
                            .numbered()
                            .into_any_element()
                    })
                    .note("Living room plays the original files. ReplayGain and fades don't apply.")
            }),
            _accent: theme::observe_accent(cx),
        }
    }
}

impl Render for SheetContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let entity = cx.entity();
        let set = |change: fn(&mut SheetContent, bool)| {
            let entity = entity.clone();
            move |value: &bool, _: &mut Window, cx: &mut gpui_kit::App| {
                let value = *value;
                entity.update(cx, |sheet, cx| {
                    change(sheet, value);
                    cx.notify();
                })
            }
        };

        let column = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .gap(look.space(Space::M))
                .flex_1()
                .min_w_0()
                .children(children)
        };

        let wash = self.wash.clone();
        let playing = self.playing;
        let entity_for_play = entity.clone();
        let type_scale = vec![
            text("kōan", TypeRole::Display, Tone::Strong, &look),
            text("albums", TypeRole::Title, Tone::Strong, &look),
            text("Low Tide Arcade", TypeRole::TitleSmall, Tone::Strong, &look),
            text("Body text, in ink.", TypeRole::Body, Tone::Ink, &look),
            text("Secondary text, muted.", TypeRole::Meta, Tone::Muted, &look),
            text(
                "The record's accent, as text.",
                TypeRole::Body,
                Tone::Accent,
                &look,
            ),
            text("Something went wrong.", TypeRole::Fine, Tone::Bad, &look),
        ];

        let buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.space(Space::M))
            .child(
                Button::new("play", ButtonKind::Prominent)
                    .label("Play")
                    .icon(Icon::Play)
                    .on_click(move |_, _, cx| {
                        let playing = !playing;
                        entity_for_play.update(cx, |sheet, cx| {
                            sheet.playing = playing;
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
            .child(Button::new("copy", ButtonKind::Bordered).label("Copy"))
            .child(Button::new("token", ButtonKind::Link).label("Find your token"))
            .child(Button::new("clear", ButtonKind::Text).label("Clear"));
        let more_buttons = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.space(Space::M))
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
            );

        let icons =
            div()
                .flex()
                .flex_wrap()
                .gap(look.space(Space::S))
                .children(Icon::ALL.iter().map(|icon| {
                    KoanIcon::new(*icon)
                        .role(TypeRole::Control)
                        .into_any_element()
                }));
        let fills = div()
            .flex()
            .items_center()
            .gap(look.space(Space::M))
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
            .child(KoanIcon::new(Icon::Play).role(TypeRole::Display));

        let controls = vec![
            Toggle::new("show-icons", "Show icons", look.icons)
                .on_change(|icons, _, cx| KoanTheme::set_icons(*icons, cx))
                .into_any_element(),
            Toggle::new("gapless", "Gapless", self.gapless)
                .on_change(set(|s, v| s.gapless = v))
                .into_any_element(),
            Toggle::new("locked", "Locked", true)
                .disabled(true)
                .into_any_element(),
            div()
                .flex()
                .gap(look.space(Space::M))
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
            Segmented::new("filter", ["All", "Favourites", "Lossless"], self.segment)
                .on_change({
                    let entity = entity.clone();
                    move |index, _, cx| {
                        let index = *index;
                        entity.update(cx, |sheet, cx| {
                            sheet.segment = index;
                            cx.notify();
                        })
                    }
                })
                .into_any_element(),
            Slider::new("level", self.level)
                .ends("1:24", "-2:17")
                .on_change({
                    let entity = entity.clone();
                    move |level, _, cx| {
                        let level = *level;
                        entity.update(cx, |sheet, cx| {
                            sheet.level = level;
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
                        entity.update(cx, |sheet, cx| {
                            sheet.buffer = buffer;
                            cx.notify();
                        })
                    }
                })
                .into_any_element(),
        ];

        let form = div()
            .flex()
            .flex_col()
            .child(SectionHeader::new("Playback"))
            .child(FormRow::new("Output").value("Built-in Output"))
            .child(FormRow::new("Name").control(div().w(px(220.0)).child(TextField::new(&self.field))))
            .child(FormRow::data("Oratory1990").value("harman"))
            .child(
                FormRow::new("Server").control(
                    Button::new("sign-out", ButtonKind::Bordered).label("Sign out"),
                ),
            )
            .child(
                div()
                    .text_role(TypeRole::Fine, &look)
                    .text_color(look.palette.muted.hsla())
                    .child("A footer that explains the section, long enough to wrap onto a second line."),
            );

        let selected_row = self.selected_row;
        let rows = div().flex().flex_col().children(
            [
                ("1", "Shoreline Pinball", "Low Tide Arcade", "3:41"),
                ("2", "Coin Return", "Low Tide Arcade", "4:02"),
                ("3", "Neon Breakwater", "Low Tide Arcade", "5:17"),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, (number, title, detail, length))| {
                let entity = entity.clone();
                ListRow::new(("row", index))
                    .number(number)
                    .title(title)
                    .detail(detail)
                    .figure(length)
                    .selected(index == selected_row)
                    .playing(index == 2)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |sheet, cx| {
                            sheet.selected_row = index;
                            cx.notify();
                        })
                    })
            }),
        );

        let nav = div()
            .flex()
            .flex_col()
            .w(px(220.0))
            .child(
                NavRow::new("nav-queue", "Queue")
                    .icon(Icon::QueueSection)
                    .selected(true),
            )
            .child(NavRow::new("nav-albums", "Albums").icon(Icon::Album))
            .child(NavRow::new("nav-artists", "Artists").icon(Icon::Artist))
            .child(NavRow::new("nav-history", "History").icon(Icon::History));

        let menu = div()
            .flex()
            .items_start()
            .gap(look.space(Space::L))
            .child(
                div().w(px(220.0)).child(
                    MenuPanel::new()
                        .child(MenuRow::new("sleep-off", "Off").checked(true))
                        .child(
                            MenuRow::new("sleep-15", "In 15 minutes")
                                .checked(false)
                                .highlighted(true),
                        )
                        .child(
                            MenuRow::new("sleep-record", "At the end of the record").checked(false),
                        )
                        .child(
                            div()
                                .h(look.hairline)
                                .my(look.space(Space::Xs))
                                .bg(look.palette.rule.hsla()),
                        )
                        .child(
                            MenuRow::new("forget", "Forget")
                                .icon(Icon::Clear)
                                .destructive(true),
                        ),
                ),
            )
            .child(self.tooltip.clone())
            .child(div().w(px(340.0)).child(self.outputs.clone()));

        let badges = div()
            .flex()
            .items_center()
            .gap(look.space(Space::S))
            .child(Badge::new("FLAC 24/96"))
            .child(Badge::new("Bit-perfect").accent(true))
            .child(Chip::new("chip-dac", "Topping E30"))
            .child(Chip::new("chip-preset", "Harman 2018"));

        let surface = div()
            .p(look.space(Space::M))
            .bg(look.palette.surface.hsla())
            .child(TextField::new(&self.empty_field).bare());

        let left = column(
            [
                type_scale,
                vec![
                    SectionHeader::new("Buttons").into_any_element(),
                    buttons.into_any_element(),
                    more_buttons.into_any_element(),
                    SectionHeader::new("Icons").into_any_element(),
                    icons.into_any_element(),
                    fills.into_any_element(),
                    SectionHeader::new("Controls").into_any_element(),
                ],
                controls,
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        let right = column(vec![
            SectionHeader::new("Form").into_any_element(),
            form.into_any_element(),
            SectionHeader::new("Rows").into_any_element(),
            rows.into_any_element(),
            SectionHeader::new("Navigation").into_any_element(),
            nav.into_any_element(),
            SectionHeader::new("Menu and tooltip").into_any_element(),
            menu.into_any_element(),
            SectionHeader::new("Badges and chips").into_any_element(),
            badges.into_any_element(),
            SectionHeader::new("Surface").into_any_element(),
            surface.into_any_element(),
            SectionHeader::new("Empty").into_any_element(),
            div()
                .h(px(200.0))
                .border(look.hairline)
                .border_color(look.palette.rule.hsla())
                .child(
                    EmptyState::new(Icon::Playlist, "No playlists")
                        .detail("Make one from the queue, or drop tracks here."),
                )
                .into_any_element(),
        ]);

        crate::components::ScrollArea::new(
            "sheet",
            div()
                .font_family(theme::FONT_FAMILY)
                .flex()
                .gap(look.space(Space::Xxl))
                .p(look.space(Space::Xxl))
                .child(left)
                .child(right),
        )
    }
}

fn text(content: &'static str, role: TypeRole, tone: Tone, look: &Look) -> AnyElement {
    div()
        .text_role(role, look)
        .text_color(look.text_color(tone))
        .child(content)
        .into_any_element()
}

/// The sample sleeve's side.
const SAMPLE: u32 = 96;

/// A sleeve for the sheet's wash and accent: a warm field and a cool one
/// over a dark ground, so both the blur and the hue weighting show.
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
