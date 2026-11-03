# Builds the Tauri shell for Linux (deb + AppImage) and Windows (NSIS, cross-
# compiled with mingw-w64; Debian 13 for NSIS >= 3.09). Build context: the repository root, so ui/dist
# (build it first) and spikes/shell are both inside.
FROM rust:1.90-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
      libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev file imagemagick \
      gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 nsis \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu
RUN --mount=type=cache,target=/usr/local/cargo/registry cargo install tauri-cli --version "^2" --locked
