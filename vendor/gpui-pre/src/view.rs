use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, AvailableSpace, Bounds, ContentMask, Context,
    Element, ElementId, Entity, EntityId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, PaintIndex, Pixels, PrepaintStateIndex, Render, RenderOnce, Size, Style,
    StyleRefinement, TextStyle, WeakEntity, window::a11y::CapturedA11y,
};
use crate::{Empty, Window};
use anyhow::Result;
use collections::FxHashSet;
use refineable::Refineable;
use std::{
    any::{TypeId, type_name},
    fmt,
    ops::Range,
};

/// A dynamically-typed view handle that can be downcast to a specific `Entity<V>`.
///
/// This is the type-erased counterpart to [`ViewElement`]: it holds an entity plus
/// a function pointer to its render, and is itself a [`View`], so embedding it as an
/// element goes through the same [`ViewElement`] machinery as any other view.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    view_type_name: &'static str,
}

impl<V: Render> From<Entity<V>> for AnyView {
    fn from(value: Entity<V>) -> Self {
        AnyView {
            entity: value.into_any(),
            render: any_view::render::<V>,
            view_type_name: type_name::<V>(),
        }
    }
}

impl AnyView {
    /// Embed this view as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is recycled from the previous frame unless
    /// [Context::notify] was called on the backing entity since it was rendered
    /// (or [Window::refresh] is called, which ignores caching).
    pub fn cached(self, style: StyleRefinement) -> ViewElement<AnyView> {
        self.into_element().cached(style)
    }

    /// Convert this to a weak handle.
    pub fn downgrade(&self) -> AnyWeakView {
        AnyWeakView {
            entity: self.entity.downgrade(),
            render: self.render,
            view_type_name: self.view_type_name,
        }
    }

    /// Convert this to a [Entity] of a specific type.
    /// If this handle does not contain a view of the specified type, returns itself in an `Err` variant.
    pub fn downcast<T: 'static>(self) -> Result<Entity<T>, Self> {
        match self.entity.downcast() {
            Ok(entity) => Ok(entity),
            Err(entity) => Err(Self {
                entity,
                render: self.render,
                view_type_name: self.view_type_name,
            }),
        }
    }

    /// Gets the [TypeId] of the underlying view.
    pub fn entity_type(&self) -> TypeId {
        self.entity.entity_type
    }

    /// The [`EntityId`] of this view.
    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

/// `AnyView` is the type-erased [`View`]: its `render` is a function pointer rather
/// than a concrete type, but it participates in the reactive graph exactly like any
/// other view via [`ViewElement`].
impl View for AnyView {
    fn entity_id(&self) -> Option<EntityId> {
        Some(self.entity.entity_id())
    }

    #[cfg(not(gpui_hot_reload))]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.render)(&self, window, cx)
    }

    // The stored function pointer was captured from the original binary, so it must be looked up
    // in the jump table to reach the patched render.
    #[cfg(gpui_hot_reload)]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        subsecond::HotFn::current(self.render).call((&self, window, cx))
    }
}

impl<V: 'static + Render> IntoElement for Entity<V> {
    type Element = ViewElement<Entity<V>>;

    fn into_element(self) -> Self::Element {
        ViewElement::with_type_name(self, type_name::<V>())
    }

    #[inline(never)]
    fn into_any_element(self) -> AnyElement {
        self.into_element().into_any()
    }
}

impl IntoElement for AnyView {
    type Element = ViewElement<AnyView>;

    fn into_element(self) -> Self::Element {
        let view_type_name = self.view_type_name;
        ViewElement::with_type_name(self, view_type_name)
    }
}

/// A weak, dynamically-typed view handle.
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    view_type_name: &'static str,
}

impl AnyWeakView {
    /// Upgrade to a strong `AnyView` handle, if the view is still alive.
    pub fn upgrade(&self) -> Option<AnyView> {
        let entity = self.entity.upgrade()?;
        Some(AnyView {
            entity,
            render: self.render,
            view_type_name: self.view_type_name,
        })
    }
}

impl<V: 'static + Render> From<WeakEntity<V>> for AnyWeakView {
    fn from(view: WeakEntity<V>) -> Self {
        AnyWeakView {
            entity: view.into(),
            render: any_view::render::<V>,
            view_type_name: type_name::<V>(),
        }
    }
}

impl PartialEq for AnyWeakView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl std::fmt::Debug for AnyWeakView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyWeakView")
            .field("entity_id", &self.entity.entity_id)
            .finish_non_exhaustive()
    }
}

mod any_view {
    use crate::{AnyElement, AnyView, App, IntoElement, Render, Window};

    pub(crate) fn render<V: 'static + Render>(
        view: &AnyView,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let view = view.clone().downcast::<V>().unwrap();
        // Record the view's Render type name so the accessibility debug dump can
        // attribute nodes to the view that produced them.
        #[cfg(debug_assertions)]
        window
            .a11y
            .view_type_names
            .insert(view.entity_id(), std::any::type_name::<V>());
        view.update(cx, |view, cx| view.render(window, cx).into_any_element())
    }
}

/// A renderable that participates in GPUI's reactive graph — the unifying model
/// behind [`Render`] and [`RenderOnce`].
///
/// When `entity_id()` returns `Some`, that id becomes the view's identity: it gets
/// a unique element-id space (so internal `use_state` / `.id(..)` never collide
/// across siblings) and `cx.notify()` on that entity re-renders only this view's
/// subtree. `None` behaves like a stateless component.
///
/// You rarely implement `View` directly. `Entity<T: Render>` and any `T: RenderOnce`
/// get a blanket impl below; implement it by hand only when a component needs both
/// parent-supplied props *and* a backing entity for identity.
pub trait View: 'static + Sized {
    /// This view's identity, if it has one. A view typically holds the backing
    /// entity as a field and returns its [`EntityId`] here.
    ///
    /// The id becomes this view's [`ElementId`], so two views keyed on the same
    /// entity must not be rendered at the same position in the element tree
    /// (e.g. as siblings under the same parent): their internal element state
    /// (`use_state`, scroll offsets, etc.) would silently collide. Nesting is
    /// fine — the id is scoped by the parent path.
    fn entity_id(&self) -> Option<EntityId>;

    /// Render this view into an element tree, consuming `self`.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}

/// A stateless component (`RenderOnce`) is a `View` with no identity.
impl<T: RenderOnce> View for T {
    fn entity_id(&self) -> Option<EntityId> {
        None
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        RenderOnce::render(self, window, cx)
    }
}

/// An entity that renders itself (`Render`) is a `View` keyed on its own id.
impl<T: Render> View for Entity<T> {
    fn entity_id(&self) -> Option<EntityId> {
        Some(Entity::entity_id(self))
    }

    #[inline]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.update(cx, |this, cx| {
            Render::render(this, window, cx).into_any_element()
        })
    }
}

impl<T: Render> Entity<T> {
    /// Embed this entity as a cached [`ViewElement`] laid out at `style`.
    ///
    /// The rendered subtree is reused until the entity is notified (or the
    /// cached bounds / text style change). Caching requires a definite size:
    /// a cached view is laid out from `style` and is *not* measured from its
    /// contents. Use [`ViewElement::new`] (or `.child(entity)`) for the
    /// uncached case.
    #[track_caller]
    pub fn cached(self, style: StyleRefinement) -> ViewElement<Entity<T>> {
        ViewElement::with_type_name(self, type_name::<T>()).cached(style)
    }
}

/// The element type for [`View`] implementations. Wraps a `View` and hooks it
/// into layout, prepaint, and paint. Constructed via [`ViewElement::new`].
#[doc(hidden)]
pub struct ViewElement<V: View> {
    view: Option<V>,
    entity_id: Option<EntityId>,
    cached_style: Option<StyleRefinement>,
    #[cfg(feature = "profiler")]
    view_type_name: &'static str,
    #[cfg(debug_assertions)]
    source: &'static core::panic::Location<'static>,
}

impl<V: View> ViewElement<V> {
    /// Wrap a [`View`] as an element.
    #[track_caller]
    pub fn new(view: V) -> Self {
        Self::with_type_name(view, type_name::<V>())
    }

    /// Wraps a view whose draw time profiling attributes to `view_type_name`,
    /// such as an entity's type rather than [`Entity`]'s.
    #[track_caller]
    #[cfg_attr(not(feature = "profiler"), expect(unused_variables))]
    pub(crate) fn with_type_name(view: V, view_type_name: &'static str) -> Self {
        let entity_id = view.entity_id();
        ViewElement {
            entity_id,
            cached_style: None,
            #[cfg(feature = "profiler")]
            view_type_name,
            view: Some(view),
            #[cfg(debug_assertions)]
            source: core::panic::Location::caller(),
        }
    }

    /// Enable caching of this view's rendered subtree, laid out at `style`.
    /// The composer supplies the layout style because caching skips rendering
    /// the contents to measure them.
    ///
    /// Crate-private on purpose: caching is only sound for entity-backed views,
    /// where [`Context::notify`] is the contract that busts the cache. A stateless
    /// view has no such contract, so a frozen subtree could never be invalidated.
    /// Reach this through [`Entity::cached`] or [`AnyView::cached`], which are
    /// entity-backed by construction.
    pub(crate) fn cached(mut self, style: StyleRefinement) -> Self {
        self.cached_style = Some(style);
        self
    }
}

impl<V: View> IntoElement for ViewElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct ViewElementState {
    prepaint_range: Range<PrepaintStateIndex>,
    // KOAN: the draws `prepaint_range` and `paint_range` were recorded in.
    // A cached ancestor reused whole carries this state into the next frame
    // without moving the ranges, which then point into a frame that is gone.
    prepaint_frame: u64,
    paint_frame: u64,
    paint_range: Range<PaintIndex>,
    cache_key: ViewElementCacheKey,
    accessed_entities: FxHashSet<EntityId>,
    /// The accessibility nodes the prepaint added, when it ran while the tree was being built.
    /// A frame that builds the tree can only reuse the view if it has them: the tree starts
    /// empty in every frame and the reused prepaint doesn't run.
    a11y: Option<CapturedA11y>,
}

struct ViewElementCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
}

#[cfg(not(gpui_hot_reload))]
#[inline]
fn render_view<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
    view.render(window, cx).into_any_element()
}

/// Every view and component render passes through this hot-patchable boundary, so the
/// patched code for that render and everything it calls runs on the next frame.
#[cfg(gpui_hot_reload)]
fn render_view<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
    #[inline(never)]
    fn render_view_impl<V: View>(view: V, window: &mut Window, cx: &mut App) -> AnyElement {
        view.render(window, cx).into_any_element()
    }

    subsecond::HotFn::current(render_view_impl::<V>).call((view, window, cx))
}

impl<V: View> Element for ViewElement<V> {
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        self.entity_id.map(ElementId::View)
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        #[cfg(debug_assertions)]
        return Some(self.source);

        #[cfg(not(debug_assertions))]
        return None;
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path: create a reactive boundary.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            let view = &mut self.view;
            let layout = request_layout_view(
                entity_id,
                self.cached_style.as_ref(),
                window,
                cx,
                &mut |window, cx| render_view(view.take().unwrap(), window, cx),
            );
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::RequestLayout,
            );
            layout
        } else {
            // Stateless path: isolate subtree via type name (no entity identity).
            request_layout_component(type_name::<V>(), window, cx, &mut |window, cx| {
                render_view(self.view.take().unwrap(), window, cx)
            })
        }
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            let prepainted = prepaint_view(
                entity_id,
                global_id,
                bounds,
                element,
                window,
                cx,
                &mut |window, cx| render_view(self.view.take().unwrap(), window, cx),
            );
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::Prepaint,
            );
            prepainted
        } else {
            // Stateless path: just prepaint the element.
            prepaint_component(type_name::<V>(), element, window, cx)
        }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        element: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(entity_id) = self.entity_id {
            // Stateful path.
            #[cfg(feature = "profiler")]
            let start = window.window_profiler.begin_view();
            paint_view(
                entity_id,
                self.cached_style.is_some(),
                global_id,
                element,
                window,
                cx,
            );
            #[cfg(feature = "profiler")]
            window.window_profiler.end_view(
                start,
                self.view_type_name,
                crate::profiler::ViewPhase::Paint,
            );
        } else {
            // Stateless path: just paint the element.
            paint_component(std::any::type_name::<V>(), element, window, cx);
        }
    }
}

/// A view that renders nothing
pub struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[inline(never)]
fn request_layout_view(
    entity_id: EntityId,
    cached_style: Option<&StyleRefinement>,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> (LayoutId, Option<AnyElement>) {
    window.with_rendered_view(entity_id, |window| {
        let caching_disabled = window.is_inspector_picking(cx);
        match cached_style {
            Some(style) if !caching_disabled => {
                let mut root_style = Style::default();
                root_style.refine(style);
                let layout_id = window.request_layout(root_style, None, cx);
                (layout_id, None)
            }
            _ => {
                let mut element = render(window, cx);
                let layout_id = element.request_layout(window, cx);
                (layout_id, Some(element))
            }
        }
    })
}

#[inline(never)]
fn request_layout_component(
    name: &'static str,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> (LayoutId, Option<AnyElement>) {
    window.with_id(ElementId::from(name), |window| {
        let mut element = render(window, cx);
        let layout_id = element.request_layout(window, cx);
        (layout_id, Some(element))
    })
}

#[inline(never)]
fn prepaint_view(
    entity_id: EntityId,
    global_id: Option<&GlobalElementId>,
    bounds: Bounds<Pixels>,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
    render: &mut dyn FnMut(&mut Window, &mut App) -> AnyElement,
) -> Option<AnyElement> {
    window.set_view_id(entity_id);
    window.with_rendered_view(entity_id, |window| {
        if let Some(mut element) = element.take() {
            element.prepaint(window, cx);
            return Some(element);
        }

        window.with_element_state::<ViewElementState, _>(
            global_id.unwrap(),
            |element_state, window| {
                let content_mask = window.content_mask();
                let text_style = window.text_style();

                if let Some(mut element_state) = element_state
                    && element_state.cache_key.bounds == bounds
                    && element_state.cache_key.content_mask == content_mask
                    && element_state.cache_key.text_style == text_style
                    && !window.dirty_views.contains(&entity_id)
                    && !window.refreshing
                    // KOAN: reused only from the frame just drawn, the one
                    // its ranges index into.
                    && element_state.prepaint_frame + 1 == window.frames_drawn
                    && element_state.paint_frame + 1 == window.frames_drawn
                    // KOAN: (#1899) after Wormhole's gpui-pre patch: a view
                    // reused while the accessibility tree is built adds the
                    // nodes it recorded, so it needs to have recorded them.
                    && (element_state.a11y.is_some() || !window.a11y.is_active())
                {
                    let prepaint_start = window.prepaint_index();
                    window.reuse_prepaint(element_state.prepaint_range.clone());
                    if window.a11y.is_active()
                        && let Some(a11y) = &element_state.a11y
                    {
                        let focus = window.focus;
                        window.a11y.replay(a11y, focus);
                    }
                    cx.entities
                        .extend_accessed(&element_state.accessed_entities);
                    let prepaint_end = window.prepaint_index();
                    element_state.prepaint_range = prepaint_start..prepaint_end;
                    element_state.prepaint_frame = window.frames_drawn;

                    return (None, element_state);
                }

                // KOAN: a view drawn again leaves the cached views inside it
                // to their own keys: one that is neither dirty nor moved is
                // reused, so a change redraws its own view and its ancestors
                // and nothing beside them. Only a refresh of the window draws
                // everything again.
                let prepaint_start = window.prepaint_index();
                let a11y_mark = window.a11y.is_active().then(|| window.a11y.mark());
                let (element, accessed_entities) = cx.detect_accessed_entities(|cx| {
                    let mut element = render(window, cx);
                    element.layout_as_root(Size::<AvailableSpace>::from(bounds.size), window, cx);
                    element.prepaint_at(bounds.origin, window, cx);
                    element
                });

                let prepaint_end = window.prepaint_index();
                let a11y = a11y_mark.map(|mark| window.a11y.capture(&mark));

                (
                    Some(element),
                    ViewElementState {
                        accessed_entities,
                        a11y,
                        prepaint_range: prepaint_start..prepaint_end,
                        prepaint_frame: window.frames_drawn,
                        paint_frame: 0,
                        paint_range: PaintIndex::default()..PaintIndex::default(),
                        cache_key: ViewElementCacheKey {
                            bounds,
                            content_mask,
                            text_style,
                        },
                    },
                )
            },
        )
    })
}

#[inline(never)]
fn prepaint_component(
    name: &'static str,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    window.with_id(ElementId::from(name), |window| {
        element.as_mut().unwrap().prepaint(window, cx);
    });
    Some(element.take().unwrap())
}

#[inline(never)]
fn paint_view(
    entity_id: EntityId,
    cached: bool,
    global_id: Option<&GlobalElementId>,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_rendered_view(entity_id, |window| {
        let caching_disabled = window.is_inspector_picking(cx);
        if cached && !caching_disabled {
            window.with_element_state::<ViewElementState, _>(
                global_id.unwrap(),
                |element_state, window| {
                    let mut element_state = element_state.unwrap();

                    let paint_start = window.paint_index();

                    if let Some(element) = element {
                        // KOAN: as in prepaint, the cached views inside are
                        // left to their own keys.
                        element.paint(window, cx);
                    } else {
                        window.reuse_paint(element_state.paint_range.clone());
                        if window.a11y.is_active()
                            && let Some(a11y) = &element_state.a11y
                        {
                            window.a11y.reuse_action_listeners(a11y);
                        }
                    }

                    let paint_end = window.paint_index();
                    element_state.paint_range = paint_start..paint_end;
                    element_state.paint_frame = window.frames_drawn; // KOAN: as prepaint_frame.

                    ((), element_state)
                },
            )
        } else {
            element.as_mut().unwrap().paint(window, cx);
        }
    });
}

#[inline(never)]
fn paint_component(
    name: &'static str,
    element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) {
    window.with_id(ElementId::Name(name.into()), |window| {
        element.as_mut().unwrap().paint(window, cx);
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use accesskit::{Action, ActionRequest, NodeId, Role, TreeId, TreeUpdate};

    use crate::{
        AnyWindowHandle, AppContext as _, Context, FocusHandle, InteractiveElement as _,
        IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, StyleRefinement,
        Styled as _, TestAppContext, Window, deferred, div, px,
    };

    /// The cached view: a button with an action listener, and a row of two more nodes. With a
    /// popup it also holds a deferred element (as a popover does) with a button of its own.
    struct Inner {
        renders: Rc<Cell<usize>>,
        clicks: Rc<Cell<usize>>,
        focus: FocusHandle,
        popup: Option<Popup>,
    }

    struct Popup {
        clicks: Rc<Cell<usize>>,
        focus: FocusHandle,
    }

    impl Render for Inner {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let clicks = self.clicks.clone();
            let popup = self.popup.as_ref().map(|popup| {
                let clicks = popup.clicks.clone();
                deferred(
                    div().id("popup").role(Role::Menu).size(px(40.)).child(
                        div()
                            .id("popup-button")
                            .role(Role::MenuItem)
                            .aria_label("Popup")
                            .track_focus(&popup.focus)
                            .on_a11y_action(Action::Click, move |_, _, _| {
                                clicks.set(clicks.get() + 1)
                            })
                            .size(px(20.)),
                    ),
                )
                .with_priority(1)
            });
            let mut root = div()
                .id("inner")
                .role(Role::Group)
                .size(px(100.))
                .child(
                    div()
                        .id("button")
                        .role(Role::Button)
                        .aria_label("Go")
                        .track_focus(&self.focus)
                        .on_a11y_action(Action::Click, move |_, _, _| clicks.set(clicks.get() + 1))
                        .size(px(20.)),
                )
                .child(div().id("label").role(Role::Label).size(px(20.)));
            if let Some(popup) = popup {
                root = root.child(popup);
            }
            root
        }
    }

    struct Outer {
        inner: crate::Entity<Inner>,
    }

    impl Render for Outer {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().id("outer").role(Role::Toolbar).size_full().child(
                self.inner
                    .clone()
                    .cached(StyleRefinement::default().size(px(100.))),
            )
        }
    }

    fn tree(window: &Window) -> TreeUpdate {
        window.last_a11y_tree_update().expect("a tree was built")
    }

    /// The ids of the nodes in `tree`, sorted, with each node's children.
    fn shape(tree: &TreeUpdate) -> Vec<(NodeId, Vec<NodeId>)> {
        let mut shape: Vec<_> = tree
            .nodes
            .iter()
            .map(|(id, node)| (*id, node.children().to_vec()))
            .collect();
        shape.sort_by_key(|(id, _)| id.0);
        shape
    }

    fn node_with_role(tree: &TreeUpdate, role: Role) -> NodeId {
        tree.nodes
            .iter()
            .find(|(_, node)| node.role() == role)
            .map(|(id, _)| *id)
            .expect("a node with the role")
    }

    fn node_with_label(tree: &TreeUpdate, label: &str) -> NodeId {
        tree.nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .map(|(id, _)| *id)
            .expect("a node with the label")
    }

    struct Fixture {
        window: AnyWindowHandle,
        renders: Rc<Cell<usize>>,
        clicks: Rc<Cell<usize>>,
        popup_clicks: Rc<Cell<usize>>,
        inner: crate::Entity<Inner>,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext) -> Self {
            Self::build(cx, false)
        }

        fn with_popup(cx: &mut TestAppContext) -> Self {
            Self::build(cx, true)
        }

        fn build(cx: &mut TestAppContext, with_popup: bool) -> Self {
            let renders = Rc::new(Cell::new(0));
            let clicks = Rc::new(Cell::new(0));
            let popup_clicks = Rc::new(Cell::new(0));
            let inner = cx.new(|cx| Inner {
                renders: renders.clone(),
                clicks: clicks.clone(),
                focus: cx.focus_handle(),
                popup: with_popup.then(|| Popup {
                    clicks: popup_clicks.clone(),
                    focus: cx.focus_handle(),
                }),
            });
            let window = cx
                .add_window({
                    let inner = inner.clone();
                    |_, _| Outer { inner }
                })
                .into();
            // Adding the window drew it once, without the tree.
            renders.set(0);
            Self {
                window,
                renders,
                clicks,
                popup_clicks,
                inner,
            }
        }

        /// Draw a frame; the tree it sent, if it built one.
        fn draw(&self, cx: &mut TestAppContext) -> Option<TreeUpdate> {
            cx.update_window(self.window, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.is_a11y_active().then(|| tree(window))
            })
            .expect("draw")
        }

        fn simulate_tree(&self, cx: &mut TestAppContext, active: bool) {
            cx.update_window(self.window, |_, window, _| {
                window.simulate_a11y_active(active)
            })
            .expect("simulate");
        }

        fn click(&self, cx: &mut TestAppContext, node: NodeId) {
            cx.update_window(self.window, |_, window, cx| {
                window.simulate_a11y_action(
                    ActionRequest {
                        action: Action::Click,
                        target_tree: TreeId::ROOT,
                        target_node: node,
                        data: None,
                    },
                    cx,
                )
            })
            .expect("click");
        }
    }

    #[gpui::test]
    fn a_reused_view_keeps_its_a11y_nodes(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        fixture.simulate_tree(cx, true);

        let first = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 1);
        let shape_first = shape(&first);
        let button = node_with_role(&first, Role::Button);
        let outer = node_with_role(&first, Role::Toolbar);
        let group = node_with_role(&first, Role::Group);
        assert!(
            first
                .nodes
                .iter()
                .any(|(id, node)| *id == outer && node.children() == [group]),
            "the cached view's top node is a child of the node it sits in"
        );

        // Nothing changed: the view's prepaint is replayed, not run, and the tree is whole.
        let second = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 1, "the view was reused");
        assert_eq!(shape(&second), shape_first);

        // A reused frame keeps the view's action listeners, frame after frame.
        fixture.click(cx, button);
        assert_eq!(fixture.clicks.get(), 1);
        let third = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 1);
        assert_eq!(shape(&third), shape_first);
        fixture.click(cx, button);
        assert_eq!(fixture.clicks.get(), 2);

        // Notified, the view renders again and the tree is the same.
        fixture.inner.update(cx, |_, cx| cx.notify());
        let fourth = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 2);
        assert_eq!(shape(&fourth), shape_first);
        fixture.click(cx, button);
        assert_eq!(fixture.clicks.get(), 3);
    }

    #[gpui::test]
    fn a_view_prepainted_without_the_tree_is_rendered_when_it_is_built(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let button;
        {
            fixture.simulate_tree(cx, true);
            let first = fixture.draw(cx).expect("a tree");
            button = node_with_role(&first, Role::Button);
        }

        // Frames without the tree reuse the view as before.
        fixture.simulate_tree(cx, false);
        assert!(fixture.draw(cx).is_none());
        assert_eq!(fixture.renders.get(), 1);
        fixture.inner.update(cx, |_, cx| cx.notify());
        assert!(fixture.draw(cx).is_none());
        assert_eq!(fixture.renders.get(), 2, "rendered while the tree was off");

        // That prepaint left no nodes to replay: the first frame with the tree renders it.
        fixture.simulate_tree(cx, true);
        let tree = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 3);
        assert_eq!(node_with_role(&tree, Role::Button), button);
        let again = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 3, "and the next one reuses it");
        assert_eq!(shape(&again), shape(&tree));
        fixture.click(cx, button);
        assert_eq!(fixture.clicks.get(), 1);

        // A view prepainted with the tree and reused without it is whole when it is built again.
        fixture.simulate_tree(cx, false);
        assert!(fixture.draw(cx).is_none());
        fixture.simulate_tree(cx, true);
        let back = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 3);
        assert_eq!(shape(&back), shape(&tree));
        fixture.click(cx, button);
        assert_eq!(fixture.clicks.get(), 2);
    }

    #[gpui::test]
    fn a_reused_view_reports_the_focus_it_holds(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        fixture.simulate_tree(cx, true);
        let button = node_with_role(&fixture.draw(cx).expect("a tree"), Role::Button);
        cx.update_window(fixture.window, |_, window, cx| {
            let focus = fixture.inner.read(cx).focus.clone();
            window.focus(&focus, cx);
        })
        .expect("focus");
        fixture.draw(cx);
        let renders = fixture.renders.get();
        let reused = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), renders, "the view was reused");
        assert_eq!(reused.focus, button);
    }

    #[gpui::test]
    fn a_deferred_popup_in_a_reused_view_keeps_its_nodes(cx: &mut TestAppContext) {
        let fixture = Fixture::with_popup(cx);
        fixture.simulate_tree(cx, true);

        let first = fixture.draw(cx).expect("a tree");
        assert_eq!(fixture.renders.get(), 1);
        let popup = node_with_label(&first, "Popup");
        let menu = node_with_role(&first, Role::Menu);
        assert!(
            first
                .nodes
                .iter()
                .any(|(id, node)| *id == menu && node.children() == [popup]),
            "the popup's button is a child of its menu"
        );
        let shape_first = shape(&first);

        cx.update_window(fixture.window, |_, window, cx| {
            let focus = match &fixture.inner.read(cx).popup {
                Some(popup) => popup.focus.clone(),
                None => unreachable!("the fixture has a popup"),
            };
            window.focus(&focus, cx);
        })
        .expect("focus");
        fixture.draw(cx);

        // Frames that reuse the view reuse the deferred draw too, and the popup stays whole.
        for round in 1..=3 {
            let renders = fixture.renders.get();
            let reused = fixture.draw(cx).expect("a tree");
            assert_eq!(
                fixture.renders.get(),
                renders,
                "round {round}: the view was reused"
            );
            assert_eq!(shape(&reused), shape_first, "round {round}");
            assert_eq!(
                reused.focus, popup,
                "round {round}: the popup's focus is reported"
            );
            fixture.click(cx, popup);
            assert_eq!(
                fixture.popup_clicks.get(),
                round,
                "round {round}: its action works"
            );
        }

        // The tree off and on again: the next frame with it is whole.
        fixture.simulate_tree(cx, false);
        assert!(fixture.draw(cx).is_none());
        fixture.simulate_tree(cx, true);
        let back = fixture.draw(cx).expect("a tree");
        assert_eq!(shape(&back), shape_first);
        fixture.click(cx, popup);
        assert_eq!(fixture.popup_clicks.get(), 4);
    }
}
