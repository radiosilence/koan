//! The app model a koan-kit app is written in, and the composition its pages
//! do: entities and their contexts, views and elements, events, actions,
//! focus, globals and tasks, and `div` with the traits that give it
//! children, layout and listeners; and the platform's services the app
//! uses (the clipboard, dropped files, the file picker). Pages build from
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
    IntoElement, KeyBinding, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement, PathPromptOptions, Pixels,
    Point, ReadGlobal, Render, RenderOnce, ScrollEvent, SharedString, Size, Stateful, Subscription,
    Task, UpdateGlobal, WeakEntity, Window, WindowHandle, WindowId, WindowVisibility, actions, div,
};

/// The text input koan-kit's fields draw (`TextField`, `FilterField`): a
/// page owns its state and listens to its events, and a field that leads a
/// list takes the arrows and Escape from it.
pub mod input {
    pub use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
}
