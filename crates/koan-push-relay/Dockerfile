# The push relay. Packages a binary built outside it, as the koan image does: CI builds
# `koan-push-relay` in release on an Ubuntu 24.04 runner per architecture and builds this with
# dist/ as the context, holding <arch>/koan-push-relay. The runtime's glibc must be at least the
# runner's, which trixie's is. Distroless: the relay needs glibc and CA certificates, nothing else.
FROM gcr.io/distroless/cc-debian13:nonroot
ARG TARGETARCH
COPY ${TARGETARCH}/koan-push-relay /usr/local/bin/koan-push-relay
EXPOSE 8080 9090
ENTRYPOINT ["/usr/local/bin/koan-push-relay"]
