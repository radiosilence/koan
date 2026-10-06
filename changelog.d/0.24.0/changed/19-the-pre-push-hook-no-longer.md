- The pre-push hook no longer runs `git add -A && git commit --amend` when `cargo fmt` changes files —
  it swept unrelated working-tree changes into the user's commit. It now fails and asks. It also runs
  `--all-targets`, matching CI, so warnings in test code stop passing the hook and failing CI.
