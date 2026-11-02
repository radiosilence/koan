//! The app model a koan-kit app is written in, and the composition its pages
//! do: entities and their contexts, views and elements, events, actions,
//! focus, globals and tasks, and `div` with the traits that give it
//! children, layout and listeners; and the platform's services the app
//! uses (the clipboard, dropped files, the file picker), and the size a
//! cached view keeps between frames (`StyleRefinement`). Pages build from
//! these and koan-kit's components.
//!
//! What a page sizes, colours or draws by hand (`px`, colours, fonts,
//! images, canvases, lists, overlays, animation) is not here: it goes
//! through the components and tokens, or through `raw` with a reason.

pub use gpui_kit::prelude::*;
pub use gpui_kit::{
    Action, AnyElement, AnyEntity, AnyView, AnyWeakEntity, AnyWindowHandle, App, AppContext,
    AsyncApp, AsyncWindowContext, Bounds, ClickEvent, ClipboardItem, Context, Div, DragMoveEvent,
    ElementId, Entity, EntityId, EventEmitter, ExternalPaths, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, KeyContext, KeyDownEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement,
    PathPromptOptions, Pixels, Point, QuitMode, ReadGlobal, Render, RenderOnce, ScrollEvent,
    SharedString, Size, Stateful, StyleRefinement, Subscription, Task, UpdateGlobal, WeakEntity,
    Window, WindowHandle, WindowId, WindowVisibility, actions, div,
};

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
