# syntax=docker/dockerfile:1.2
FROM ghcr.io/radiosilence/nano-web:latest

LABEL org.opencontainers.image.source="https://github.com/radiosilence/koan" \
  org.opencontainers.image.url="https://koan.rocks" \
  org.opencontainers.image.title="koan-site" \
  org.opencontainers.image.description="koan.rocks"

COPY public /public
ENV PORT=3000
EXPOSE 3000
