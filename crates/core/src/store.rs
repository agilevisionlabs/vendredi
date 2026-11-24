use anyhow::{Context, Result};
use std::path::Path;
use tokio::runtime::{Builder, Runtime};
use turso::{Connection, IntoParams, Row};

pub struct Store {
    runtime: Runtime,
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let db = runtime.block_on(
            turso::Builder::new_local(path.to_str().context("invalid database path")?).build(),
        )?;
        Ok(Self {
            runtime,
            conn: db.connect()?,
        })
    }

    pub fn exec(&self, sql: &str, args: impl IntoParams) -> Result<i64> {
        self.runtime.block_on(self.conn.execute(sql, args))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn batch(&self, sql: &str) -> Result<()> {
        Ok(self.runtime.block_on(self.conn.execute_batch(sql))?)
    }

    pub fn query(&self, sql: &str, args: impl IntoParams) -> Result<Vec<Row>> {
        self.runtime.block_on(async {
            let mut cursor = self.conn.query(sql, args).await?;
            let mut rows = Vec::new();
            while let Some(row) = cursor.next().await? {
                rows.push(row);
            }
            Ok(rows)
        })
    }

    pub fn transaction<T>(&self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        self.exec("BEGIN IMMEDIATE", ())?;
        let result = f(self).and_then(|value| {
            self.exec("COMMIT", ())?;
            Ok(value)
        });
        if result.is_err() {
            self.exec("ROLLBACK", ())?;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn committed_bytes_survive_interrupted_transaction() -> Result<()> {
        const FIXTURE_PATH: &str = "VENDREDI_TEST_INTERRUPTED_WRITER";
        if let Some(path) = std::env::var_os(FIXTURE_PATH) {
            let store = Store::open(Path::new(&path))?;
            store.exec("CREATE TABLE fixture(bytes BLOB)", ())?;
            store.transaction(|s| s.exec("INSERT INTO fixture VALUES (?)", [vec![0, 255]]))?;
            store.exec("BEGIN IMMEDIATE", ())?;
            store.exec("INSERT INTO fixture VALUES (X'01')", ())?;
            // Terminate with the connection and write transaction still live.
            std::process::exit(77);
        }
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("traffic.db");
        let status = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "store::tests::committed_bytes_survive_interrupted_transaction",
            ])
            .env(FIXTURE_PATH, &path)
            .status()?;
        assert_eq!(status.code(), Some(77));
        let s = Store::open(&path)?;
        let rows = s.query("SELECT bytes FROM fixture", ())?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<Vec<u8>>(0)?, [0, 255]);
        assert_eq!(
            s.query("PRAGMA integrity_check", ())?[0].get::<String>(0)?,
            "ok"
        );
        Ok(())
    }
}
