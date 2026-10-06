- **The Linux audio callback took a mutex on every buffer.** The rtrb consumer was wrapped in a
  `std::sync::Mutex` so it could move into cpal's `FnMut` callback — but `rtrb::Consumer` is already
  `Send` and the callback bound is `FnMut + Send`, so a by-value capture was always enough. The lock
  cost an atomic read-modify-write per callback on the real-time thread, and its `try_lock` failure
  path filled the buffer with silence: an audible dropout reachable only through a contention that
  could not occur.
