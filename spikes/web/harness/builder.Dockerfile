# Rust toolchain for the spike: native binaries + wasm32 + wasm-bindgen.
FROM rust:1.90-bookworm
RUN rustup target add wasm32-unknown-unknown \
 && apt-get update && apt-get install -y --no-install-recommends binaryen brotli && rm -rf /var/lib/apt/lists/*
# Must match the wasm-bindgen version in Cargo.lock.
ARG WASM_BINDGEN=0.2.92
RUN cargo install wasm-bindgen-cli --version ${WASM_BINDGEN} --locked
