- **A panicking background job poisoned the job registry** for every later lookup, and a poisoned
  decode cursor silently stopped the gapless lookahead — stalling the queue with no error rather than
  failing loudly. Both now use `parking_lot`, which the project's own primitive table specifies.
