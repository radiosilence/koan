# The demo server's library: freely licensed releases, fetched from library.tsv
# (folder, URL, SHA-256) and checked against it, under /music. The chart's demo
# mode copies /music into its pod with this image's `cp`; see CREDITS.md.
FROM alpine:3.21 AS fetch
RUN apk add --no-cache curl
COPY library.tsv /library.tsv
RUN set -eu; tab="$(printf '\t')"; \
    grep -v '^#' /library.tsv | while IFS="$tab" read -r dir url sha; do \
      file="/music/$dir/$(basename "$url" | sed 's/%20/ /g')"; \
      mkdir -p "/music/$dir"; \
      curl -fsSL --retry 5 --retry-all-errors -o "$file" "$url"; \
      echo "$sha  $file" | sha256sum -c -; \
    done; \
    chmod -R a+rX /music

FROM busybox:1.37
COPY --from=fetch /music /music
COPY CREDITS.md /music/CREDITS.md
