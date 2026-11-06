//! A window's tabs along its top, as the Mac's Settings draws them, and the
//! one line at a window's foot that says what is under way or how it went.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;

use super::{AnimatedLayer, Ease, Handler};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Motion, Space, Tone, TypeRole, label};

/// The width of a bar's gliding rule, in points at 100 %.
const BAR_RULE: f32 = 28.0;

/// A bar's rule: its offset track, the bar's width last laid out, and the
/// tab it last stood under with its offset.
struct BarRule {
    animated: super::Animated,
    width: std::rc::Rc<std::cell::Cell<Pixels>>,
    at: Option<(usize, f32)>,
}

impl Default for BarRule {
    fn default() -> Self {
        let animated = super::Animated::new();
        super::layer::standing(&animated);
        Self {
            animated,
            width: Default::default(),
            at: None,
        }
    }
}

/// The tabs, centred: each its glyph over its label in `fine`, lowercase,
/// `muted`, and the chosen one in the accent and underlined. The glyph only
/// when icons are on.
#[derive(IntoElement)]
pub struct TabStrip {
    tabs: Vec<(ElementId, Icon, SharedString, bool, Handler<ClickEvent>)>,
    bar: bool,
}

impl TabStrip {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            bar: false,
        }
    }

    /// A phone's tab bar, as the iOS app's: the words alone, the tabs
    /// sharing the bar's width, each a thumb's height to tap.
    pub fn bar(mut self) -> Self {
        self.bar = true;
        self
    }

    /// A tab: its glyph, its title in the app's words, whether it is the one
    /// showing, and what choosing it does.
    pub fn tab(
        mut self,
        icon: Icon,
        title: impl AsRef<str>,
        chosen: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let title = label(title);
        let id = ElementId::Name(title.clone());
        self.tabs
            .push((id, icon, title, chosen, Arc::new(on_click)));
        self
    }
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TabStrip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let prism = crate::theme::KoanTheme::prism_gradient(cx);
        let bar = self.bar;
        // A bar's one rule, gliding to the chosen tab on the GPU rather than
        // each tab drawing its own: the tabs share the bar's width, which the
        // last frame measured.
        let glide = bar.then(|| {
            let chosen = self.tabs.iter().position(|t| t.3).unwrap_or(0);
            let count = self.tabs.len().max(1);
            let state = window.use_keyed_state("tab-bar-rule", cx, |_, _| BarRule::default());
            let rule = look.spaced(BAR_RULE);
            let scale = window.scale_factor();
            let (animated, width) = {
                let state = state.read(cx);
                (state.animated.clone(), state.width.clone())
            };
            let tab = width.get() / count as f32;
            let target = f32::from(tab * chosen as f32 + (tab - rule) / 2.0) * scale;
            let moved = state.update(cx, |state, _| {
                let was = state.at.replace((chosen, target));
                was.map(|(tab, _)| tab != chosen)
            });
            match moved {
                Some(true) => animated.to(1, target, Motion::SETTLE, Ease::QuickOut),
                _ => animated.hold(1, target),
            }
            let measure = canvas(
                move |bounds, _, _| width.set(bounds.size.width),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            div().absolute().inset_0().child(measure).child(
                div()
                    .absolute()
                    .left_0()
                    .bottom(px(8.0))
                    .child(AnimatedLayer::new(
                        &animated,
                        div().w(rule).h(look.hairline * 2.).bg(look.accent.hsla()),
                    )),
            )
        });
        div()
            .relative()
            .children(glide)
            .flex()
            .justify_center()
            .when(bar, |strip| strip.w_full().min_h(px(49.0)).items_center())
            .when(!bar, |strip| {
                strip
                    .gap(look.space(Space::L))
                    .px(look.space(Space::M))
                    .pb(look.space(Space::Xs))
            })
            .children(self.tabs.into_iter().map(|(id, icon, title, chosen, run)| {
                let color = look.text_color(if chosen { Tone::Accent } else { Tone::Muted });
                let prism = chosen.then_some(prism).flatten();
                let rule = super::child(&id, "prism");
                div()
                    .id(id)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(look.space(Space::Xs))
                    .when(bar, |item| {
                        item.flex_1()
                            .justify_center()
                            .min_h(crate::theme::tokens::HIT)
                    })
                    .when(!bar, |item| {
                        item.px(look.space(Space::S)).pt(look.space(Space::S))
                    })
                    .cursor_pointer()
                    .text_role(TypeRole::Fine, &look)
                    .text_color(color)
                    .when(look.icons && !bar, |item| {
                        item.child(KoanIcon::new(icon).role(TypeRole::Title).color(color))
                    })
                    .child(
                        div()
                            .relative()
                            .pb(px(5.0))
                            .border_b(look.hairline)
                            .border_color(look.accent.hsla().opacity(
                                if chosen && prism.is_none() && !bar {
                                    1.0
                                } else {
                                    0.0
                                },
                            ))
                            .child(title)
                            // While the rainbow is drawn, the chosen tab's
                            // rule is the palette, a little heavier.
                            .children(prism.map(|brat| {
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(px(2.0))
                                    .child(super::PrismFill::new(rule, brat))
                            })),
                    )
                    .on_click(move |e, window, cx| run(e, window, cx))
            }))
    }
}

/// What a window is doing or did last: a glyph and two lines at most, in
/// `fine`, along its foot over a rule. Its room is kept when it is empty, so
/// nothing jumps when it speaks.
#[derive(IntoElement)]
pub struct StatusLine {
    text: SharedString,
    tone: Tone,
    icon: Option<Icon>,
}

impl StatusLine {
    pub fn new(text: impl Into<SharedString>, tone: Tone, icon: Option<Icon>) -> Self {
        Self {
            text: text.into(),
            tone,
            icon,
        }
    }
}

impl RenderOnce for StatusLine {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let color = look.text_color(self.tone);
        div()
            .flex()
            .items_center()
            .gap(look.space(Space::Xs))
            .min_h(look.line_height(TypeRole::Fine) * 2. + look.space(Space::S) * 2.)
            .px(look.space(Space::L))
            .py(look.space(Space::S))
            .border_t(look.hairline)
            .border_color(look.palette.rule.hsla())
            .text_role(TypeRole::Fine, &look)
            .text_color(color)
            .children(
                self.icon
                    .map(|icon| KoanIcon::new(icon).role(TypeRole::Fine).color(color)),
            )
            .child(div().line_clamp(2).child(self.text))
    }
}
