//! Postgres connection pool and migrations (spec §7.1, §7.2). Postgres from
//! day one — no SQLite abstraction layer — because the design depends on
//! `FOR UPDATE SKIP LOCKED`.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}
