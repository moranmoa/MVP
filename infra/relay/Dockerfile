FROM rust:1.90-bookworm AS build
# Docker on the dev PC is shared by several builds; cap parallel rustc jobs.
ARG CARGO_BUILD_JOBS=4
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 relay && mkdir /data && chown relay /data
COPY --from=build /src/target/release/relay-node /usr/local/bin/relay-node
USER relay
VOLUME /data
EXPOSE 4001/tcp 4001/udp 4002/tcp
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["relay-node", "healthcheck"]
ENTRYPOINT ["relay-node"]
