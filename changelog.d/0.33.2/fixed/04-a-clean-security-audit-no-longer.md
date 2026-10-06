- **A clean security audit no longer reports itself as a failed build.** The audit job reports through a GitHub check run, which is an API write, and this repository hands workflows a read-only token by default — so the job found nothing, tried to say so, and failed with "Resource not accessible by integration" on every push to main. It asks for the one permission it needs now.

