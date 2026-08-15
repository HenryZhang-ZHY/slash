//! The `slash-server` binary entrypoint. All logic lives in the library
//! crate (`lib.rs`) so the same modules are reachable from the integration
//! tests under `tests/`; this file is intentionally a thin `#[tokio::main]`
//! wrapper around `slash_server::run`.

#[tokio::main]
async fn main() {
    slash_server::run().await;
}
