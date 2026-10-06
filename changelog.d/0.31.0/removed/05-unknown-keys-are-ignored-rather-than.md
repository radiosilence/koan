Unknown keys are ignored rather than rejected, so a stale config still loads. Two
renames change behaviour silently and are worth grepping your config for:
`[graphql] subsonic_port` (now `[subsonic] port`) and `[discovery]
analysis_on_scan` (now `[library] analyze_on_scan`). The others were inert.
