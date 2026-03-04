use crossterm::event::{KeyEvent, MouseEvent};

pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
}
