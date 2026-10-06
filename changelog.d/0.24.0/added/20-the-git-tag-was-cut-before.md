- **The git tag was cut before crates.io.** Once tagged, `check-version` saw the release as done and
  set `should_release=false` forever, so crates.io could never be retried. crates.io now publishes
  first and the tag marks a release that actually shipped.
