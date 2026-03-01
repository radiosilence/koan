use std::time::Duration;

use crossterm::event::{self, Event as CtEvent, KeyEvent, MouseEvent};

pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Tick,
}

/// Poll crossterm events with a tick interval.
pub fn poll(tick_rate: Duration) -> std::io::Result<Event> {
    if event::poll(tick_rate)? {
        match event::read()? {
            CtEvent::Key(key) => Ok(Event::Key(key)),
            CtEvent::Mouse(mouse) => Ok(Event::Mouse(mouse)),
            CtEvent::Paste(text) => Ok(Event::Paste(text)),
            _ => Ok(Event::Tick),
        }
    } else {
        Ok(Event::Tick)
    }
}
