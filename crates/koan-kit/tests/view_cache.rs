//! Cached views drawn again inside a dirty ancestor (#1880, #1899): a
//! cached view whose state came into a frame through its ancestor's reuse
//! is drawn afresh the next time that ancestor is drawn, never reusing
//! ranges into a frame that is gone.

use gpui::{
    AnyWindowHandle, AppContext as _, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, StyleRefinement, Styled, TestAppContext,
    Window, div, px,
};

/// A cached view showing a line of text, counting its renders.
struct Leaf {
    text: &'static str,
    renders: usize,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().size_full().child(self.text)
    }
}

/// A cached view holding two cached leaves.
struct Parent {
    leaves: [Entity<Leaf>; 2],
}

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .children(self.leaves.iter().map(|leaf| {
                leaf.clone()
                    .cached(StyleRefinement::default().w(px(100.)).h(px(20.)))
            }))
    }
}

/// The window's root: some lines of its own, then the cached parent.
struct Root {
    lines: usize,
    parent: Entity<Parent>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .children((0..self.lines).map(|i| div().h(px(20.)).child(format!("line {i}"))))
            // Placed where it stays, whatever the lines above it do, so it
            // can be reused whole while the frame's text shifts around it.
            .child(
                div().absolute().top(px(400.)).left_0().child(
                    self.parent
                        .clone()
                        .cached(StyleRefinement::default().w(px(200.)).h(px(20.))),
                ),
            )
    }
}

fn draw(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .expect("draw window");
}

/// The leaves' render counts.
fn renders(leaves: &[Entity<Leaf>; 2], cx: &mut TestAppContext) -> (usize, usize) {
    cx.read(|cx| (leaves[0].read(cx).renders, leaves[1].read(cx).renders))
}

fn board(cx: &mut TestAppContext) -> ([Entity<Leaf>; 2], gpui::WindowHandle<Root>) {
    let leaves = [
        cx.new(|_| Leaf {
            text: "left",
            renders: 0,
        }),
        cx.new(|_| Leaf {
            text: "right",
            renders: 0,
        }),
    ];
    let parent = cx.new(|_| Parent {
        leaves: leaves.clone(),
    });
    let root = cx.add_window(|_, _| Root { lines: 4, parent });
    (leaves, root)
}

/// Frames come as the app's do: each change ends in one drawn frame (the
/// test platform draws a dirty window as an update finishes), and nothing
/// draws a frame with nothing changed.
#[gpui::test]
fn a_cached_view_carried_by_its_ancestor_is_drawn_afresh(cx: &mut TestAppContext) {
    let (leaves, root) = board(cx);
    cx.run_until_parked();
    assert_eq!(renders(&leaves, cx), (1, 1));

    // The root draws again with fewer lines of its own: the parent is reused
    // whole, carrying both leaves' state into this frame without moving the
    // ranges they recorded in the first.
    root.update(cx, |root, _, cx| {
        root.lines = 0;
        cx.notify();
    })
    .expect("update root");
    assert_eq!(renders(&leaves, cx), (1, 1), "the parent is reused whole");

    // One leaf changes, so the parent draws again. The other leaf's ranges
    // index the first frame, which is gone: it must be drawn afresh rather
    // than reused, which panicked in `reuse_layouts`.
    leaves[0].update(cx, |_, cx| cx.notify());
    assert_eq!(renders(&leaves, cx), (2, 2));

    // In the next frame the untouched leaf was drawn in the frame before, so
    // it is reused: a change that redraws everything fails here.
    leaves[0].update(cx, |_, cx| cx.notify());
    assert_eq!(renders(&leaves, cx), (3, 2));
    leaves[0].update(cx, |_, cx| cx.notify());
    assert_eq!(renders(&leaves, cx), (4, 2));
}

/// A leaf that names itself to a screen reader.
struct Named(&'static str);

impl Render for Named {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.0)
            .role(gpui::Role::Button)
            .aria_label(self.0)
            .size_full()
            .child(self.0)
    }
}

struct NamedParent([Entity<Named>; 2]);

impl Render for NamedParent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().children(self.0.iter().map(|leaf| {
            leaf.clone()
                .cached(StyleRefinement::default().w(px(100.)).h(px(20.)))
        }))
    }
}

struct NamedRoot {
    lines: usize,
    parent: Entity<NamedParent>,
}

impl Render for NamedRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .children((0..self.lines).map(|i| div().h(px(20.)).child(format!("line {i}"))))
            .child(
                div().absolute().top(px(400.)).left_0().child(
                    self.parent
                        .clone()
                        .cached(StyleRefinement::default().w(px(200.)).h(px(20.))),
                ),
            )
    }
}

/// A cached view reused whole stays in the accessibility tree, and so do
/// the cached views inside it, carried or reused on their own.
#[gpui::test]
fn reused_views_stay_in_the_accessibility_tree(cx: &mut TestAppContext) {
    let named = [cx.new(|_| Named("play")), cx.new(|_| Named("next"))];
    let parent = cx.new(|_| NamedParent(named.clone()));
    let root = cx.add_window(|_, _| NamedRoot { lines: 2, parent });
    root.update(cx, |_, window, _| window.set_a11y_active_for_test(true))
        .expect("activate a11y");
    let tree = |cx: &mut TestAppContext| {
        root.update(cx, |_, window, _| {
            window.debug_a11y_tree_json().unwrap_or_default()
        })
        .expect("read tree")
    };
    let both = |json: &str| json.contains("\"play\"") && json.contains("\"next\"");

    // Drawn once with the tree built.
    root.update(cx, |root, _, cx| {
        root.lines = 3;
        cx.notify();
    })
    .expect("update root");
    assert!(both(&tree(cx)), "drawn: {}", tree(cx));

    // The parent reused whole.
    root.update(cx, |root, _, cx| {
        root.lines = 1;
        cx.notify();
    })
    .expect("update root");
    assert!(both(&tree(cx)), "parent reused: {}", tree(cx));

    // One leaf redrawn, its sibling carried, then reused.
    for _ in 0..2 {
        named[0].update(cx, |_, cx| cx.notify());
        assert!(both(&tree(cx)), "a leaf redrawn: {}", tree(cx));
    }
}

/// A cached group of cached leaves, for the churn below.
struct Group {
    leaves: Vec<Entity<Leaf>>,
    renders: usize,
}

impl Render for Group {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div()
            .size_full()
            .flex()
            .children(self.leaves.iter().map(|leaf| {
                leaf.clone()
                    .cached(StyleRefinement::default().w(px(40.)).h(px(20.)))
            }))
    }
}

/// Lines of its own that change in number, then the cached groups.
struct Board {
    lines: usize,
    groups: Vec<Entity<Group>>,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .children((0..self.lines).map(|i| div().h(px(20.)).child(format!("line {i}"))))
            .children(self.groups.iter().map(|group| {
                group
                    .clone()
                    .cached(StyleRefinement::default().w(px(400.)).h(px(20.)))
            }))
    }
}

/// What a hover storm does to a page of cached rows: leaves, their groups and
/// the page notifying in any order, a frame drawn after each, as the pointer
/// crossing controls redraws them while their siblings stay cached. Every
/// frame must draw without reusing a range from a frame that is gone.
#[gpui::test]
fn a_hover_storm_across_cached_views_draws_every_frame(cx: &mut TestAppContext) {
    const GROUPS: usize = 6;
    const LEAVES: usize = 5;
    const STEPS: usize = 600;
    let groups: Vec<Entity<Group>> = (0..GROUPS)
        .map(|_| {
            let leaves = (0..LEAVES)
                .map(|_| {
                    cx.new(|_| Leaf {
                        text: "control",
                        renders: 0,
                    })
                })
                .collect();
            cx.new(|_| Group { leaves, renders: 0 })
        })
        .collect();
    let board = cx.add_window(|_, _| Board {
        lines: 3,
        groups: groups.clone(),
    });
    let window: AnyWindowHandle = board.into();
    draw(window, cx);

    // A fixed sequence, so a failure reproduces.
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    for _ in 0..STEPS {
        match next(10) {
            // The page itself, with a different number of lines of its own.
            0 => {
                let lines = next(8);
                board
                    .update(cx, |board, _, cx| {
                        board.lines = lines;
                        cx.notify();
                    })
                    .expect("update board");
            }
            // A row redrawn as a whole.
            1 | 2 => {
                let group = &groups[next(GROUPS)];
                group.update(cx, |_, cx| cx.notify());
            }
            // A control under the pointer, alone or with a neighbour.
            _ => {
                for _ in 0..1 + next(2) {
                    let group = groups[next(GROUPS)].clone();
                    let leaf = cx.read(|cx| group.read(cx).leaves[next(LEAVES)].clone());
                    leaf.update(cx, |_, cx| cx.notify());
                }
            }
        }
        // Sometimes two changes land in one frame, sometimes a frame passes
        // with none.
        if next(4) != 0 {
            draw(window, cx);
        }
    }
    draw(window, cx);
}

/// Where the app is, as a navigator: an entity the stage observes.
struct Route {
    album: bool,
}

/// A page made by the stage when it first shows: its rows arrive from a
/// read in a spawned task, which notifies when they land.
struct AlbumPage {
    rows: Option<usize>,
    /// Renders that drew the rows.
    drawn_with_rows: usize,
}

impl AlbumPage {
    fn new(cx: &mut Context<Self>) -> Self {
        let read = cx
            .background_executor()
            .timer(std::time::Duration::from_millis(5));
        cx.spawn(async move |page, cx| {
            read.await;
            page.update(cx, |page, cx| {
                page.rows = Some(12);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            rows: None,
            drawn_with_rows: 0,
        }
    }
}

impl Render for AlbumPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.rows.is_some() {
            self.drawn_with_rows += 1;
        }
        div()
            .size_full()
            .child(format!("{} rows", self.rows.unwrap_or(0)))
    }
}

/// The stage: makes the page as it renders for the album route.
struct Stage {
    route: Entity<Route>,
    page: Option<Entity<AlbumPage>>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.route.read(cx).album && self.page.is_none() {
            self.page = Some(cx.new(AlbumPage::new));
        }
        div().size_full().children(
            self.page
                .clone()
                .map(|page| page.cached(StyleRefinement::default().size_full())),
        )
    }
}

/// The window's content, cached as the shell's is.
struct Content {
    stage: Entity<Stage>,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.stage
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

struct Shell {
    content: Entity<Content>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.content
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

/// A page two cached views down, made when its stage renders and filled by
/// a spawned read, draws its rows when the read notifies, though its stage
/// drew again in between and reused it: no input, no refresh, nothing but
/// the notify asks for the frame. The stage made the page, so the page's
/// own entity was first read in the stage's render.
#[gpui::test]
fn a_page_filled_by_a_spawned_read_draws_without_input(cx: &mut TestAppContext) {
    let route = cx.new(|_| Route { album: false });
    let stage = cx.new(|cx| {
        let route_handle = route.clone();
        cx.observe(&route_handle, |_, _, cx| cx.notify()).detach();
        Stage {
            route: route_handle,
            page: None,
        }
    });
    let content = cx.new(|_| Content {
        stage: stage.clone(),
    });
    let _window = cx.add_window(|_, _| Shell { content });
    cx.run_until_parked();

    route.update(cx, |route, cx| {
        route.album = true;
        cx.notify();
    });
    cx.run_until_parked();
    let page = stage
        .read_with(cx, |stage, _| stage.page.clone())
        .expect("the stage made the page as it rendered");

    // The stage draws again before the read lands (the navigator notifies
    // more than once for a move), and the page, unchanged, is reused.
    route.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    cx.executor()
        .advance_clock(std::time::Duration::from_millis(10));
    cx.run_until_parked();
    let (rows, drawn) = page.read_with(cx, |page, _| (page.rows, page.drawn_with_rows));
    assert_eq!(rows, Some(12), "the read landed");
    assert!(drawn > 0, "the rows landed but no frame drew them");
}

/// A view that asks to be drawn again while it is being drawn, as a list
/// measuring its rows does: once.
struct Remeasures {
    renders: usize,
    model: Entity<Counter>,
}

/// An entity a view updates while it draws.
struct Counter {
    value: usize,
}

impl Render for Remeasures {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        if self.renders == 1 {
            cx.notify();
            self.model.update(cx, |model, cx| {
                model.value += 1;
                cx.notify();
            });
        }
        let value = self.model.read(cx).value;
        div().size_full().child(format!("{value}"))
    }
}

struct RemeasureShell {
    view: Entity<Remeasures>,
}

impl Render for RemeasureShell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            self.view
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
    }
}

/// A notify made while the window draws is not lost: the view is drawn by
/// the next frame, which nothing but the notify asks for, exactly once, and
/// an entity notified then reaches its observers.
#[gpui::test]
fn a_notify_during_a_draw_draws_the_next_frame(cx: &mut TestAppContext) {
    let model = cx.new(|_| Counter { value: 0 });
    let observed = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = observed.clone();
    cx.update(|cx| {
        cx.observe(&model, move |_, _| seen.set(seen.get() + 1))
            .detach()
    });
    let view = cx.new(|_| Remeasures {
        renders: 0,
        model: model.clone(),
    });
    let _window = cx.add_window(|_, _| RemeasureShell { view: view.clone() });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.renders),
        2,
        "the view asked to be drawn again while drawing"
    );
    assert_eq!(observed.get(), 1, "the model's observer heard its notify");
}
