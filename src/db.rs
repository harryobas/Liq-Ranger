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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_url() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.db");
        (dir, format!("sqlite://{}", path.display()))
    }

    #[tokio::test]
    async fn connect_creates_schema() {
        let (_dir, url) = temp_db_url();
        let pool = connect(&url).await.expect("connect");

        let liquidation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'liquidations'",
        )
        .fetch_one(&pool)
        .await
        .expect("liquidations table");
        let distribution_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'distributions'",
        )
        .fetch_one(&pool)
        .await
        .expect("distributions table");

        assert_eq!(liquidation_count, 1);
        assert_eq!(distribution_count, 1);
    }
}
