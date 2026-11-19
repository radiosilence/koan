//! Layout and type on the theme's scale, for what a page lays out itself:
//! `Stack`, a flex container whose gap and insets are steps of `Space`, and
//! `Text`, a run of text in a type role and a tone. Each is a `div` that
//! reads the window's `Look` as it lays out, so a page names a step or a
//! role and never a length or a colour; everything else a `div` takes
//! (flex, alignment, ids, handlers) goes on as it would on the `div`.

use gpui::prelude::FluentBuilder;
use gpui::{
    A11ySubtreeBuilder, AnyElement, App, Bounds, Div, DivFrameState, Element, ElementId,
    GlobalElementId, Hitbox, InspectorElementId, InteractiveElement, Interactivity, IntoElement,
    LayoutId, ParentElement, Pixels, StyleRefinement, Styled, Window, accesskit, div,
};

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Measure, Space, Tone, TypeRole};

/// The theme's part of a `Stack` or `Text`, applied as it lays out: the
/// gap, the measure and the insets, as the theme's steps.
#[doc(hidden)]
#[derive(Clone, Copy, Default)]
pub struct Spacing {
    gap: Option<Space>,
    measure: Option<Measure>,
    top: Option<Space>,
    right: Option<Space>,
    bottom: Option<Space>,
    left: Option<Space>,
}

impl Spacing {
    fn apply(self, mut div: Div, look: &Look) -> Div {
        if let Some(gap) = self.gap {
            div = div.gap(look.space(gap));
        }
        if let Some(measure) = self.measure {
            div = div.w_full().min_w_0().max_w(look.measure(measure));
        }
        if let Some(top) = self.top {
            div = div.pt(look.space(top));
        }
        if let Some(right) = self.right {
            div = div.pr(look.space(right));
        }
        if let Some(bottom) = self.bottom {
            div = div.pb(look.space(bottom));
        }
        if let Some(left) = self.left {
            div = div.pl(look.space(left));
        }
        div
    }
}

/// Insets on the spacing scale, shared by `Stack` and `Text`.
pub trait Inset: Sized {
    /// The element's spacing, which the methods below set.
    #[doc(hidden)]
    fn spacing(&mut self) -> &mut Spacing;

    /// The same inset on every edge.
    fn inset(mut self, space: Space) -> Self {
        let s = self.spacing();
        (s.top, s.right, s.bottom, s.left) = (Some(space), Some(space), Some(space), Some(space));
        self
    }

    /// The same inset on the leading and trailing edges.
    fn inset_x(mut self, space: Space) -> Self {
        let s = self.spacing();
        (s.left, s.right) = (Some(space), Some(space));
        self
    }

    /// The same inset on the top and bottom edges.
    fn inset_y(mut self, space: Space) -> Self {
        let s = self.spacing();
        (s.top, s.bottom) = (Some(space), Some(space));
        self
    }

    /// The inset on the top edge.
    fn inset_top(mut self, space: Space) -> Self {
        self.spacing().top = Some(space);
        self
    }

    /// The inset on the bottom edge.
    fn inset_bottom(mut self, space: Space) -> Self {
        self.spacing().bottom = Some(space);
        self
    }

    /// The inset on the left edge.
    fn inset_left(mut self, space: Space) -> Self {
        self.spacing().left = Some(space);
        self
    }

    /// The inset on the right edge.
    fn inset_right(mut self, space: Space) -> Self {
        self.spacing().right = Some(space);
        self
    }
}

/// A flex container on the spacing scale: `Stack::column().space(Space::L)`.
pub struct Stack {
    div: Div,
    children: Vec<AnyElement>,
    spacing: Spacing,
    role: Option<TypeRole>,
    tone: Option<Tone>,
}

impl Stack {
    /// Children one under another.
    pub fn column() -> Self {
        Self::of(div().flex().flex_col())
    }

    /// Children side by side.
    pub fn row() -> Self {
        Self::of(div().flex())
    }

    /// Children side by side, wrapping onto further lines when they do not
    /// fit: a set of buttons on a narrow window.
    pub fn wrapping() -> Self {
        Self::of(div().flex().flex_wrap())
    }

    fn of(div: Div) -> Self {
        Self {
            div,
            children: Vec::new(),
            spacing: Spacing::default(),
            role: None,
            tone: None,
        }
    }

    /// The gap between children.
    pub fn space(mut self, space: Space) -> Self {
        self.spacing.gap = Some(space);
        self
    }

    /// As wide as its parent up to `measure`, centred in what is left.
    pub fn measure(mut self, measure: Measure) -> Self {
        self.spacing.measure = Some(measure);
        self.div = std::mem::replace(&mut self.div, div()).mx_auto();
        self
    }

    /// The type role its text takes, unless a child says otherwise.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.role = Some(role);
        self
    }

    /// The tone its text takes, unless a child says otherwise.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }
}

impl Inset for Stack {
    fn spacing(&mut self) -> &mut Spacing {
        &mut self.spacing
    }
}

/// A run of text in a type role, in a tone or the one it sits in:
/// `Text::new(TypeRole::Meta).tone(Tone::Muted).child(words)`.
pub struct Text {
    div: Div,
    children: Vec<AnyElement>,
    spacing: Spacing,
    role: TypeRole,
    tone: Option<Tone>,
    picked: bool,
    icon: Option<Icon>,
}

impl Text {
    /// Text in `role`, in the tone it sits in until `tone` says otherwise.
    pub fn new(role: TypeRole) -> Self {
        Self {
            div: div(),
            children: Vec::new(),
            spacing: Spacing::default(),
            role,
            tone: None,
            picked: false,
            icon: None,
        }
    }

    /// The tone it is drawn in.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// As wide as its parent up to `measure`, from the leading edge.
    pub fn measure(mut self, measure: Measure) -> Self {
        self.spacing.measure = Some(measure);
        self
    }

    /// A glyph before the words, in their role and tone, where icons are
    /// shown: a mark that says what kind of line it is (a check, a warning).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// On a picked row: in `on_selection`, whatever the tone.
    pub fn picked(mut self, picked: bool) -> Self {
        self.picked = picked;
        self
    }
}

impl Inset for Text {
    fn spacing(&mut self) -> &mut Spacing {
        &mut self.spacing
    }
}

fn typeset(mut div: Div, role: Option<TypeRole>, tone: Option<Tone>, picked: bool, look: &Look) -> Div {
    if let Some(role) = role {
        div = div.text_role(role, look);
    }
    if picked {
        div = div.text_color(look.palette.on_selection.hsla());
    } else if let Some(tone) = tone {
        div = div.text_color(look.text_color(tone));
    }
    div
}

/// Lays the element's `div` out with the theme applied, and otherwise is
/// that `div`.
macro_rules! themed_div {
    ($ty:ty, |$this:ident, $look:ident| $theme:expr) => {
        impl Styled for $ty {
            fn style(&mut self) -> &mut StyleRefinement {
                self.div.style()
            }
        }

        impl ParentElement for $ty {
            fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
                self.children.extend(elements)
            }
        }

        impl InteractiveElement for $ty {
            fn interactivity(&mut self) -> &mut Interactivity {
                self.div.interactivity()
            }
        }

        impl IntoElement for $ty {
            type Element = Self;

            fn into_element(self) -> Self {
                self
            }
        }

        impl Element for $ty {
            type RequestLayoutState = DivFrameState;
            type PrepaintState = Option<Hitbox>;

            fn id(&self) -> Option<ElementId> {
                Element::id(&self.div)
            }

            fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
                Element::source_location(&self.div)
            }

            fn a11y_role(&self) -> Option<accesskit::Role> {
                Element::a11y_role(&self.div)
            }

            fn write_a11y_info(&self, node: &mut accesskit::Node) {
                Element::write_a11y_info(&self.div, node)
            }

            fn a11y_synthetic_children(
                &mut self,
                prepaint: &mut Self::PrepaintState,
                builder: &mut A11ySubtreeBuilder,
            ) {
                Element::a11y_synthetic_children(&mut self.div, prepaint, builder)
            }

            fn request_layout(
                &mut self,
                id: Option<&GlobalElementId>,
                inspector_id: Option<&InspectorElementId>,
                window: &mut Window,
                cx: &mut App,
            ) -> (LayoutId, Self::RequestLayoutState) {
                let $look = Look::of(window, cx);
                let $this = &mut *self;
                let themed = $theme;
                self.div = themed.children(std::mem::take(&mut self.children));
                Element::request_layout(&mut self.div, id, inspector_id, window, cx)
            }

            fn prepaint(
                &mut self,
                id: Option<&GlobalElementId>,
                inspector_id: Option<&InspectorElementId>,
                bounds: Bounds<Pixels>,
                state: &mut Self::RequestLayoutState,
                window: &mut Window,
                cx: &mut App,
            ) -> Self::PrepaintState {
                Element::prepaint(&mut self.div, id, inspector_id, bounds, state, window, cx)
            }

            fn paint(
                &mut self,
                id: Option<&GlobalElementId>,
                inspector_id: Option<&InspectorElementId>,
                bounds: Bounds<Pixels>,
                state: &mut Self::RequestLayoutState,
                prepaint: &mut Self::PrepaintState,
                window: &mut Window,
                cx: &mut App,
            ) {
                Element::paint(&mut self.div, id, inspector_id, bounds, state, prepaint, window, cx)
            }
        }
    };
}

themed_div!(Stack, |this, look| {
    let div = std::mem::replace(&mut this.div, div());
    typeset(this.spacing.apply(div, &look), this.role, this.tone, false, &look)
});

themed_div!(Text, |this, look| {
    let div = std::mem::replace(&mut this.div, div());
    let div = typeset(this.spacing.apply(div, &look), Some(this.role), this.tone, this.picked, &look);
    match this.icon.filter(|_| look.icons) {
        Some(icon) => div
            .flex()
            .items_center()
            .gap(look.space(Space::Xs))
            .child(
                KoanIcon::new(icon).role(this.role).map(|glyph| {
                    if this.picked {
                        glyph.color(look.palette.on_selection.hsla())
                    } else {
                        glyph.tone(this.tone.unwrap_or(Tone::Ink))
                    }
                }),
            ),
        None => div,
    }
});
