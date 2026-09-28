pub mod helpers;
#[cfg(feature = "redb")]
pub mod redb;
#[cfg(feature = "sqlite")]
pub mod sqlite;

use crate::config::types::JobConfig;
use crate::scraper::extractor::ExtractedItem;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::RwLock;

pub use helpers::Record;

pub enum Backend {
    #[cfg(feature = "redb")]
    Redb(redb::RedbBackend),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteBackend),
}

pub struct Db {
    backend: RwLock<Option<Backend>>,
}

impl Db {
    #[cfg(all(feature = "redb", not(feature = "sqlite")))]
    fn create_backend(path: &str) -> Result<Backend> {
        Ok(Backend::Redb(redb::RedbBackend::open(path)?))
    }

    #[cfg(feature = "sqlite")]
    fn create_backend(path: &str) -> Result<Backend> {
        Ok(Backend::Sqlite(sqlite::SqliteBackend::open(path)?))
    }

    pub fn open(path: &str) -> Result<Self> {
        Ok(Self {
            backend: RwLock::new(Some(Self::create_backend(path)?)),
        })
    }

    pub fn close(&self) -> Result<()> {
        let backend = self
            .backend
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        match backend {
            None => Ok(()),
            #[cfg(feature = "redb")]
            Some(Backend::Redb(b)) => b.close(),
            #[cfg(feature = "sqlite")]
            Some(Backend::Sqlite(b)) => b.close(),
        }
    }

    fn with_backend<T>(&self, f: impl FnOnce(&Backend) -> Result<T>) -> Result<T> {
        let guard = self.backend.read().unwrap_or_else(|e| e.into_inner());
        let backend = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("database is closed"))?;
        f(backend)
    }

    pub fn get_records_for_job_paginated(
        &self,
        job_id: &str,
        limit: usize,
        after_timestamp: Option<u64>,
    ) -> Result<Vec<Record>> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.get_records_for_job_paginated(job_id, limit, after_timestamp),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.get_records_for_job_paginated(job_id, limit, after_timestamp),
        })
    }

    pub fn get_all_records(
        &self,
        limit_per_job: Option<usize>,
    ) -> Result<HashMap<String, Vec<Record>>> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.get_all_records(limit_per_job),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.get_all_records(limit_per_job),
        })
    }

    pub fn batch_check_and_insert(
        &self,
        job: &JobConfig,
        items: Vec<ExtractedItem>,
    ) -> Result<Vec<ExtractedItem>> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.batch_check_and_insert(job, items),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.batch_check_and_insert(job, items),
        })
    }

    pub fn lua_set(&self, key: &str, value: &str) -> Result<()> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_set(key, value),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_set(key, value),
        })
    }

    pub fn lua_get(&self, key: &str) -> Result<Option<String>> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_get(key),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_get(key),
        })
    }

    pub fn lua_delete(&self, key: &str) -> Result<()> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_delete(key),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_delete(key),
        })
    }

    pub fn lua_incr(&self, key: &str, default: i64, delta: i64) -> Result<i64> {
        self.with_backend(|backend| match backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_incr(key, default, delta),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_incr(key, default, delta),
        })
    }

    #[cfg(feature = "sqlite")]
    pub fn db_query(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<HashMap<String, sqlite::SqlValue>>> {
        self.with_backend(|backend| match backend {
            Backend::Sqlite(b) => b.db_query(sql, params),
            #[cfg(feature = "redb")]
            Backend::Redb(_) => anyhow::bail!("db_query is only available in the sqlite backend"),
        })
    }

    #[cfg(feature = "sqlite")]
    pub fn db_exec(&self, sql: &str, params: &[rusqlite::types::Value]) -> Result<u64> {
        self.with_backend(|backend| match backend {
            Backend::Sqlite(b) => b.db_exec(sql, params),
            #[cfg(feature = "redb")]
            Backend::Redb(_) => anyhow::bail!("db_exec is only available in the sqlite backend"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_close_is_idempotent_and_blocks_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        let db = Db::open(path.to_str().unwrap()).unwrap();

        db.lua_set("k", "v").unwrap();
        assert_eq!(db.lua_get("k").unwrap().unwrap(), "v");

        db.close().unwrap();
        db.close().unwrap(); // idempotent

        let err = db.lua_get("k").unwrap_err();
        assert!(err.to_string().contains("database is closed"));

        let err = db.lua_set("k", "v2").unwrap_err();
        assert!(err.to_string().contains("database is closed"));
    }
}
