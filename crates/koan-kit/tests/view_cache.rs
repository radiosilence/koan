//! Cached views drawn again inside a dirty ancestor (#1880, #1899): a
//! cached view whose state came into a frame through its ancestor's reuse
//! is drawn afresh the next time that ancestor is drawn, never reusing
//! ranges into a frame that is gone.

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, ParentElement, Render,
    StyleRefinement, Styled, TestAppContext, Window, div, px,
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
            .children((0..self.lines).map(|i| div().h(px(20.)).child(format!("line {i}"))))
            .child(
                self.parent
                    .clone()
                    .cached(StyleRefinement::default().w(px(200.)).h(px(20.))),
            )
    }
}

fn draw(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .expect("draw window");
}

#[gpui_kit::test]
fn a_cached_view_carried_by_its_ancestor_is_drawn_afresh(cx: &mut TestAppContext) {
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
    let root = cx.add_window(|_, _| Root {
        lines: 4,
        parent: parent.clone(),
    });
    let window: AnyWindowHandle = root.into();

    draw(window, cx);

    // The root draws again with fewer lines of its own: the parent is reused
    // whole, carrying both leaves' state into this frame without moving the
    // ranges they recorded in the first.
    root.update(cx, |root, _, cx| {
        root.lines = 0;
        cx.notify();
    })
    .expect("update root");
    draw(window, cx);

    // One leaf changes, so the parent draws again. The other leaf's ranges
    // index the first frame, which is gone: it must be drawn afresh rather
    // than reused, which panicked in `reuse_layouts`.
    leaves[0].update(cx, |_, cx| cx.notify());
    draw(window, cx);

    let renders = |cx: &mut TestAppContext| {
        cx.read(|cx| (leaves[0].read(cx).renders, leaves[1].read(cx).renders))
    };
    assert_eq!(renders(cx), (2, 2));

    // In the next frame the untouched leaf is reused again.
    leaves[0].update(cx, |_, cx| cx.notify());
    draw(window, cx);
    assert_eq!(renders(cx), (3, 2));
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
#[gpui_kit::test]
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
