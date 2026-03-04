use crossterm::event::{KeyEvent, MouseEvent};

#[allow(dead_code)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
}
