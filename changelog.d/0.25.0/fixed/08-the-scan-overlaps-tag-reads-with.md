- **The scan overlaps tag reads with database writes.** Chunking reads and writes with a barrier between them left the disk idle for every write and the CPU idle for every read; reads now stream down a bounded channel while the main thread commits.

