# Base images are pinned by digest (multi-arch index), not just by tag, so a
# moved tag cannot change what we build. To update: `docker buildx imagetools
# inspect <image:tag>` and paste the new digest.
FROM rust:1.90-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f AS build
# Docker on the dev PC is shared by several builds; cap parallel rustc jobs.
ARG CARGO_BUILD_JOBS=4
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# --locked: exactly the dependency versions in the committed Cargo.lock.
# The tests run here so the CI image build covers them.
RUN cargo test --release --locked && cargo build --release --locked

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
RUN useradd --system --uid 10001 relay && mkdir /data && chown relay /data && chmod 700 /data
COPY --from=build /src/target/release/relay-node /usr/local/bin/relay-node
USER relay
VOLUME /data
EXPOSE 4001/tcp 4001/udp 4002/tcp
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["relay-node", "healthcheck"]
ENTRYPOINT ["relay-node"]
