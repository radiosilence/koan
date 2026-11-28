use super::types::Conn;
use async_graphql::connection::Edge;

// ---------------------------------------------------------------------------
// Pagination — uses usize as cursor (async-graphql has built-in impl)
// ---------------------------------------------------------------------------

/// Page size when the client asks for no `first`.
///
/// Unbounded, one `{ tracks { edges { node { ... } } } }` would materialise the
/// whole library as rows, as GraphQL values and as serialised JSON at once.
pub(super) const DEFAULT_PAGE: usize = 50;

/// Ceiling on `first`, so a client cannot ask for the library in one response.
///
/// Large enough that walking a whole library takes a handful of requests rather
/// than a hundred (a 60,000-track library is six pages), small enough that one
/// page of tracks stays a few megabytes of JSON.
pub(super) const MAX_PAGE: usize = 10_000;

/// Clamp a client-supplied `first` into the servable range. Negative values are
/// floored at zero rather than wrapping to a request for the entire library.
pub(super) fn page_size(first: Option<i32>) -> usize {
    match first {
        Some(f) => (f.max(0) as usize).min(MAX_PAGE),
        None => DEFAULT_PAGE,
    }
}

/// Index of the first row after the given cursor.
pub(super) fn page_offset(after: Option<&str>) -> usize {
    after
        .and_then(|c| c.parse::<usize>().ok())
        .map(|i| i.saturating_add(1))
        .unwrap_or(0)
}

/// Paginate an already-materialised list.
pub(super) fn paginate<T: async_graphql::OutputType>(
    items: Vec<T>,
    after: Option<String>,
    first: Option<i32>,
) -> async_graphql::Result<Conn<T>> {
    let total = items.len();
    let start = page_offset(after.as_deref()).min(total);
    let end = start.saturating_add(page_size(first)).min(total);

    let mut conn = Conn::new(start > 0, end < total);
    for (i, item) in items.into_iter().enumerate().skip(start).take(end - start) {
        conn.edges.push(Edge::new(i, item));
    }
    Ok(conn)
}

/// Wrap a page that SQL already windowed. `rows` may hold one extra row beyond
/// `limit`, which is how the next-page flag is derived without a `COUNT(*)`.
pub(super) fn paginate_window<T: async_graphql::OutputType>(
    mut rows: Vec<T>,
    offset: usize,
    limit: usize,
) -> Conn<T> {
    let has_next = rows.len() > limit;
    rows.truncate(limit);
    let mut conn = Conn::new(offset > 0, has_next);
    for (i, item) in rows.into_iter().enumerate() {
        conn.edges.push(Edge::new(offset + i, item));
    }
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_defaults_clamps_and_floors() {
        assert_eq!(page_size(None), DEFAULT_PAGE);
        assert_eq!(page_size(Some(-5)), 0);
        assert_eq!(page_size(Some(2_000)), 2_000);
        assert_eq!(page_size(Some(i32::MAX)), MAX_PAGE);
    }
}
