- **Secret round-trip leak in config save** — `save()` on a merged Config would serialize secrets from `config.local.toml` and env vars back into `config.toml`. Callers now use `Config::update_base()` which reads only `config.toml`, applies the mutation, and writes back without leaking sensitive fields

