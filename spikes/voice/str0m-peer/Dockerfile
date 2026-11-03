# str0m participant for the voice bench. Build context: spikes/voice/str0m-peer
#   --build-arg FEATURES=apm  adds WebRTC's echo cancellation / noise
#   suppression / gain control (C++ build; needs meson >= 1.1, so Debian 13).
FROM rust:1.90-trixie AS build
ARG FEATURES=""
RUN apt-get update && apt-get install -y --no-install-recommends libasound2-dev libopus-dev pkg-config cmake meson ninja-build clang && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target,id=str0m-target-${FEATURES} \
    cargo build --release --locked -j 4 --features "${FEATURES}" && cp target/release/str0m-peer /usr/local/bin/

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends libasound2t64 libopus0 libstdc++6 iproute2 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/str0m-peer /usr/local/bin/
# netem first (needs NET_ADMIN), then one participant per container.
CMD ["sh", "-c", "[ -n \"$NETEM\" ] && tc qdisc replace dev eth0 root netem $NETEM; exec str0m-peer --signal ws://signal:8080 --room \"$ROOM\" --name \"s-$(hostname | cut -c1-6)\" --src \"$SRC\" --duration \"$(( ${END_AT:-0} > 0 ? END_AT - $(date +%s) : DURATION ))\" --warmup \"$WARMUP\" --results /results"]
