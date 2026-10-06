- **WAV `LIST INFO` tags are not read** — Symphonia 0.6.1's WAV reader parses the chunk into a metadata log and then builds the reader from `external_data` instead, discarding it. Only reachable for WAVs lofty cannot parse, since lofty reads these tags on the happy path; such files fall back to a filename-derived title.

