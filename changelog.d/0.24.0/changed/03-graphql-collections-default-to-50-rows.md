- **GraphQL collections default to 50 rows and cap at 500.** A query with no `first` used to return
  the entire collection, so `{ tracks { edges { node { title } } } }` materialised a whole library as
  rows, as GraphQL values and as serialised JSON at once. Clients that relied on the unbounded form
  must paginate with `first`/`after`.
