//! Shared test-only support. Postgres-backed tests are gated on
//! `SLASH_TEST_DATABASE_URL` and skip cleanly when it is absent (plan M4) —
//! `test_database_url` is the one place that decides "skip", so every test
//! module does the same early-return-if-`None` dance rather than each
//! reinventing it.

pub fn test_database_url() -> Option<String> {
    std::env::var("SLASH_TEST_DATABASE_URL").ok()
}
