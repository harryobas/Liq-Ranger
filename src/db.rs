use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::{str::FromStr, time::Duration};

pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    let pool_timeout = Duration::from_secs(60);
    let pool_max_connections = 1;

    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(pool_timeout);

    let pool = SqlitePoolOptions::new()
        .max_connections(pool_max_connections)
        .idle_timeout(pool_timeout)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./db").run(&pool).await?;

    Ok(pool)
}