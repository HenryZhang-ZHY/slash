# Slash control-plane image (plan M7). Two stages: build with the full Rust
# toolchain, then ship only the compiled binary plus the CA bundle it needs
# to reach the GitHub API over TLS (rustls + rustls-native-certs, so no
# libssl is required at runtime).
#
# Builder and runtime pin the same Debian codename (bookworm) so the glibc
# the binary links against at build time matches the one it runs against.

FROM rust:1.88-slim-bookworm AS builder
WORKDIR /app

# `ring`/`aws-lc-rs` (pulled in transitively by rustls) build C code, which
# the slim base image does not ship a compiler for.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY migrations migrations

# Migrations are embedded into the binary at compile time by
# `sqlx::migrate!("../../migrations")` (crates/slash-server/src/db.rs) — the
# runtime image never needs the migrations directory itself.
RUN cargo build --release --bin slash-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 slash

COPY --from=builder /app/target/release/slash-server /usr/local/bin/slash-server

# The GitHub App private key (SLASH_GITHUB_PRIVATE_KEY_PATH) is supplied at
# deploy time as a mounted, read-only, mode-0400 file — see
# docs/user/deployment.md — never baked into the image.
USER slash
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/slash-server"]
