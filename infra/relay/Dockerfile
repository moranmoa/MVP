FROM rust:1.83-bookworm AS build
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
ENTRYPOINT ["relay-node"]
