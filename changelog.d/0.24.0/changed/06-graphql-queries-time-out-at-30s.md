- **GraphQL queries time out at 30s, shed load past 64 in flight (503) and survive a panicking
  resolver.** The concurrency limit alone queued the surplus, so an overloaded server answered
  everyone slowly instead of telling the excess to come back. Subscriptions are exempt from the
  timeout.

