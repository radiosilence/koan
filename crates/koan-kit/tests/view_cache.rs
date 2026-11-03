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
        div().size_full().flex().children(self.leaves.iter().map(|leaf| {
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
