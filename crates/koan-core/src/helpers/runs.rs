//! Which rows of a list are one record under one heading.
//!
//! A queue or a playlist is in the order someone chose, so a record is a run
//! of rows next to each other, never gathered from across the list: two
//! visits to one record are two runs.

/// A row's place among the records of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunRole {
    /// On no record: a row of its own.
    Alone,
    /// A record of one track titled as the record, as a single is: a row of
    /// its own.
    Single,
    /// The first row of a record's run, under the record's heading.
    Opens,
    /// A later row of the run above.
    Continues,
}

/// What a row says about the record it is on.
#[derive(Debug, Clone, Copy)]
pub struct RecordOf<'a> {
    /// The library's record, where the row is a library track.
    pub album_id: Option<i64>,
    pub album: &'a str,
    pub album_artist: &'a str,
    pub title: &'a str,
}

impl RecordOf<'_> {
    /// Two records can share a title and artist, as editions do; the id tells
    /// them apart where both rows have one.
    fn same(&self, other: &Self) -> bool {
        match (self.album_id, other.album_id) {
            (Some(a), Some(b)) => a == b,
            _ => self.album == other.album && self.album_artist == other.album_artist,
        }
    }

    fn titled_as_record(&self) -> bool {
        let fold = |s: &str| s.trim().to_lowercase();
        fold(self.title) == fold(self.album)
    }
}

/// Each row's place among the records of `rows`, in order.
pub fn record_runs(rows: &[RecordOf]) -> Vec<RunRole> {
    let mut roles = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        let first = &rows[i];
        if first.album.is_empty() {
            roles.push(RunRole::Alone);
            i += 1;
            continue;
        }
        let len = rows[i..].iter().take_while(|r| r.same(first)).count();
        if len == 1 && first.titled_as_record() {
            roles.push(RunRole::Single);
        } else {
            roles.push(RunRole::Opens);
            roles.extend(std::iter::repeat_n(RunRole::Continues, len - 1));
        }
        i += len;
    }
    roles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(album_id: Option<i64>, album: &'a str, title: &'a str) -> RecordOf<'a> {
        RecordOf {
            album_id,
            album,
            album_artist: "Artist",
            title,
        }
    }

    #[test]
    fn rows_next_to_each_other_on_one_record_are_one_run() {
        use RunRole::*;
        let rows = [
            row(Some(1), "Record", "One"),
            row(Some(1), "Record", "Two"),
            row(None, "", "Loose"),
            row(Some(2), "Song", " song "),
            row(Some(3), "Other", "Three"),
            row(Some(1), "Record", "Again"),
        ];
        assert_eq!(
            record_runs(&rows),
            [Opens, Continues, Alone, Single, Opens, Opens]
        );
    }

    #[test]
    fn an_id_tells_editions_apart_and_a_missing_one_falls_back_to_the_names() {
        use RunRole::*;
        let rows = [
            row(Some(1), "Record", "One"),
            row(Some(2), "Record", "Two"),
            row(None, "Record", "Three"),
            row(Some(2), "Record", "Four"),
        ];
        assert_eq!(record_runs(&rows), [Opens, Opens, Continues, Continues]);
    }
}
