# koan as a headless server: GraphQL and Subsonic on 4000, and MCP over HTTP
# on 8081 when KOAN_MCP_BIND is set. No sound card is needed; playback
# mutations fail cleanly without one, and everything else works.
FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends libasound2-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p koan-cli

FROM debian:bookworm-slim
# ALSA and D-Bus are linked by the audio backend and media-key support.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libasound2 libdbus-1-3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home koan
COPY --from=build /src/target/release/koan /usr/local/bin/koan
USER 1000
# Config, database and auth keys; mount a volume here.
ENV KOAN_CONFIG_DIR=/config
EXPOSE 4000 8081
ENTRYPOINT ["koan", "--headless", "--bind", "0.0.0.0"]
