//! The app model a koan-kit app is written in, and the composition its pages
//! do: entities and their contexts, views and elements, events, actions,
//! focus, globals and tasks, and `div` with the traits that give it
//! children, layout and listeners; and the platform's services the app
//! uses (the clipboard, dropped files, the file picker), and the size a
//! cached view keeps between frames (`StyleRefinement`). Pages build from
//! these and koan-kit's components.
//!
//! What a page sizes, colours or draws by hand (`px`, colours, fonts,
//! canvases, lists, overlays, animation) is not here: it goes through the
//! components and tokens, or through `raw` with a reason. Sleeves decoded
//! for the components to draw are in `image`.

pub use gpui_kit::prelude::*;
pub use gpui_kit::{
    Action, AnyElement, AnyEntity, AnyView, AnyWeakEntity, AnyWindowHandle, App, AppContext,
    AsyncApp, AsyncWindowContext, Bounds, ClickEvent, ClipboardItem, Context, Div, DragMoveEvent,
    ElementId, Entity, EntityId, EventEmitter, ExternalPaths, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, KeyContext, KeyDownEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement,
    PathPromptOptions, Pixels, Point, QuitMode, ReadGlobal, Render, RenderOnce, ScrollEvent,
    SharedString, Size, Stateful, StyleRefinement, Subscription, Task, UpdateGlobal, WeakEntity,
    Window, WindowBounds, WindowHandle, WindowId, WindowVisibility, actions, div,
};

/// What the app asks of the platform beyond its windows' content: starting
/// up, the menu bar and the Dock's menu, which system this is, the keymap
/// as the shortcuts sheet and the tests read it, and how a trackpad's
/// scroll is phased.
pub mod platform {
    pub use gpui_kit::{
        KeyBindingContextPredicate, Keymap, Menu, MenuItem, OperatingSystem, OsAction,
        SystemMenuType, TouchPhase, application, init,
    };
}

/// A sleeve as the renderer holds it: decoded off the main thread by the
/// app's cover caches and handed to `Cover`, `TileSleeve` and the wash; its
/// average colour, worked out from its pixels; the renderer's id for it.
pub mod image {
    pub use gpui_kit::{Image, ImageFormat, ImageId, RenderImage, Rgba};
}

/// What a test of a page or the app drives it with.
#[cfg(any(test, feature = "test-support"))]
pub mod test {
    pub use gpui_kit::{TestAppContext, VisualTestContext, test};
}

/// What a page tells a screen reader where no component says it for the
/// page: a landmark's role (the sidebar as navigation, the stage as main), a
/// tile's role and the action it answers, a region announced as it changes.
/// Set beside the element's id with `role`, `aria_label` and `aria_live`.
pub mod a11y {
    pub use gpui_kit::accesskit::Live;
    pub use gpui_kit::{AccessibleAction, Role};
}

/// The text input koan-kit's fields draw (`TextField`, `FilterField`): a
/// page owns its state and listens to its events, and a field that leads a
/// list takes the arrows and Escape from it.
pub mod input {
    pub use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
}
