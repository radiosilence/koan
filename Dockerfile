# koan as a headless server: GraphQL, Subsonic and MCP on 4000. No sound card is needed; playback
# mutations fail cleanly without one, and everything else works.
#
# Packages a binary built outside it: CI builds `koan-cli` in release on an Ubuntu 24.04 runner per
# architecture (the `linux-build` job) and places each at dist/<arch>/koan, the same binaries a
# release attaches. Trixie's glibc is newer than the runner's, which the binary needs.
FROM debian:trixie-slim
ARG TARGETARCH
# ALSA and D-Bus are linked by the audio backend and media-key support; ffmpeg
# transcodes for Subsonic clients that ask for a lower bitrate.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libasound2t64 libdbus-1-3 ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home koan
COPY dist/${TARGETARCH}/koan /usr/local/bin/koan
USER 1000
# Config, database and auth keys; mount a volume here.
ENV KOAN_CONFIG_DIR=/config
EXPOSE 4000
ENTRYPOINT ["koan", "--headless", "--bind", "0.0.0.0"]
