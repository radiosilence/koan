//! The queue as rows: a heading above each record's run, the run's tracks
//! beneath it, and tracks on no record or standing alone. The runs are the
//! engine's (`QueueItem::run`); nothing here compares names.

use koan_ffi::{QueueItem, RunRole};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    /// A record's heading, standing for the `len` queue items from `first`.
    Heading { first: usize, len: usize },
    /// One queue item. `grouped` when a heading above it names its record.
    Track { ix: usize, grouped: bool },
}

impl Row {
    /// The queue items this row stands for, as a range of indexes.
    pub fn items(self) -> std::ops::Range<usize> {
        match self {
            Row::Heading { first, len } => first..first + len,
            Row::Track { ix, .. } => ix..ix + 1,
        }
    }
}

pub fn build(items: &[QueueItem]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len() + items.len() / 8);
    let mut ix = 0;
    while ix < items.len() {
        match items[ix].run {
            RunRole::Opens => {
                let len = 1 + items[ix + 1..]
                    .iter()
                    .take_while(|item| item.run == RunRole::Continues)
                    .count();
                rows.push(Row::Heading { first: ix, len });
                rows.extend((ix..ix + len).map(|ix| Row::Track { ix, grouped: true }));
                ix += len;
            }
            // A continuation with no opening row before it: the engine never
            // sends one, but a row is still a row.
            RunRole::Continues | RunRole::Alone | RunRole::Single => {
                rows.push(Row::Track { ix, grouped: false });
                ix += 1;
            }
        }
    }
    rows
}

/// Where a drop lands for a pointer over `row`: before it in its upper half,
/// after it in its lower half. After the last row is `rows.len()`, the end of
/// the queue.
pub fn insertion_point(row: usize, lower_half: bool) -> usize {
    row + usize::from(lower_half)
}

/// Where something from outside the queue dropped before row `dest` goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// After this queue item: the one just before the row's first item, so
    /// a drop under a heading lands at the top of its record.
    After(usize),
    /// Above the first item, which nothing can be placed after.
    Top,
    /// Past the last row.
    End,
}

pub fn place(rows: &[Row], dest: usize) -> Place {
    match rows.get(dest).map(|row| row.items().start) {
        None => Place::End,
        Some(0) => Place::Top,
        Some(start) => Place::After(start - 1),
    }
}

/// Where a drop before row `dest` puts the moving items: before the first
/// item of the first row at or after it that is not itself moving, or, when
/// every row below is moving, after the last item that stays.
///
/// Anchored to the row dropped onto rather than the one above it, because
/// only that can say "at the very top", and a drop on a heading lands above
/// the record rather than under its first track.
pub fn drop_target(
    rows: &[Row],
    dest: usize,
    moving: &[String],
    items: &[QueueItem],
) -> Option<(String, bool)> {
    let moving: std::collections::HashSet<&str> = moving.iter().map(String::as_str).collect();
    let is_moving = |ix: usize| moving.contains(items[ix].queue_item_id.as_str());
    let below = rows
        .get(dest..)
        .unwrap_or_default()
        .iter()
        .find(|row| !row.items().any(is_moving));
    if let Some(row) = below {
        return Some((items[row.items().start].queue_item_id.clone(), false));
    }
    let last = (0..items.len()).rev().find(|&ix| !is_moving(ix))?;
    Some((items[last].queue_item_id.clone(), true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_ffi::EntryStatus;

    fn item(id: &str, run: RunRole) -> QueueItem {
        QueueItem {
            queue_item_id: id.into(),
            track_id: None,
            album_id: None,
            artist_id: None,
            album_artist_id: None,
            title: id.into(),
            artist: String::new(),
            album_artist: String::new(),
            artist_credit: Vec::new(),
            album_artist_credit: Vec::new(),
            credit_differs: false,
            run,
            album: String::new(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            status: EntryStatus::Queued,
            playlist_entry_id: None,
            failure_reason: None,
            on_server: false,
            on_disk: true,
        }
    }

    fn queue() -> Vec<QueueItem> {
        vec![
            item("a1", RunRole::Opens),
            item("a2", RunRole::Continues),
            item("s", RunRole::Single),
            item("b1", RunRole::Opens),
            item("b2", RunRole::Continues),
            item("b3", RunRole::Continues),
        ]
    }

    #[test]
    fn a_run_has_a_heading_and_a_single_stands_alone() {
        let rows = build(&queue());
        assert_eq!(
            rows,
            vec![
                Row::Heading { first: 0, len: 2 },
                Row::Track {
                    ix: 0,
                    grouped: true
                },
                Row::Track {
                    ix: 1,
                    grouped: true
                },
                Row::Track {
                    ix: 2,
                    grouped: false
                },
                Row::Heading { first: 3, len: 3 },
                Row::Track {
                    ix: 3,
                    grouped: true
                },
                Row::Track {
                    ix: 4,
                    grouped: true
                },
                Row::Track {
                    ix: 5,
                    grouped: true
                },
            ]
        );
    }

    #[test]
    fn a_drop_from_outside_follows_the_item_before_the_row() {
        let track = |ix| Row::Track { ix, grouped: true };
        let rows = [
            Row::Heading { first: 0, len: 3 },
            track(0),
            track(1),
            track(2),
            Row::Heading { first: 3, len: 2 },
            track(3),
            track(4),
        ];
        assert_eq!(place(&rows, 0), Place::Top);
        // Under the first heading: still the top of the queue.
        assert_eq!(place(&rows, 1), Place::Top);
        assert_eq!(place(&rows, 2), Place::After(0));
        // Above the second record, and under its heading: after the first.
        assert_eq!(place(&rows, 4), Place::After(2));
        assert_eq!(place(&rows, 5), Place::After(2));
        assert_eq!(place(&rows, 7), Place::End);
    }

    #[test]
    fn a_drop_on_a_heading_lands_above_its_record() {
        let items = queue();
        let rows = build(&items);
        let moving = vec!["s".to_string()];
        assert_eq!(
            drop_target(&rows, 0, &moving, &items),
            Some(("a1".into(), false))
        );
        assert_eq!(
            drop_target(&rows, 4, &moving, &items),
            Some(("b1".into(), false))
        );
    }

    #[test]
    fn a_drop_onto_the_rows_being_moved_skips_past_them() {
        let items = queue();
        let rows = build(&items);
        let moving = vec!["a1".to_string(), "a2".to_string()];
        // Rows 0..3 are the record being moved; the single is the first to stay.
        assert_eq!(
            drop_target(&rows, 1, &moving, &items),
            Some(("s".into(), false))
        );
    }

    #[test]
    fn the_last_rows_lower_half_is_the_end_of_the_queue() {
        let items = queue();
        let rows = build(&items);
        let last = rows.len() - 1;
        assert_eq!(insertion_point(last, true), rows.len());
        let moving = vec!["a1".to_string()];
        assert_eq!(
            drop_target(&rows, insertion_point(last, true), &moving, &items),
            Some(("b3".into(), true))
        );
        assert_eq!(
            drop_target(&rows, insertion_point(last, false), &moving, &items),
            Some(("b3".into(), false))
        );
    }

    #[test]
    fn a_drop_past_the_end_goes_after_the_last_item_that_stays() {
        let items = queue();
        let rows = build(&items);
        let moving = vec!["a1".to_string()];
        assert_eq!(
            drop_target(&rows, rows.len(), &moving, &items),
            Some(("b3".into(), true))
        );
        let moving = vec!["b3".to_string()];
        assert_eq!(
            drop_target(&rows, 7, &moving, &items),
            Some(("b2".into(), true))
        );
    }
}
