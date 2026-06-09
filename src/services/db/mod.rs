pub mod helpers;
#[cfg(feature = "redb")]
pub mod redb;
#[cfg(feature = "sqlite")]
pub mod sqlite;

use crate::config::types::JobConfig;
use crate::scraper::extractor::ExtractedItem;
use anyhow::Result;
use std::collections::HashMap;

pub use helpers::Record;

pub enum Backend {
    #[cfg(feature = "redb")]
    Redb(redb::RedbBackend),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteBackend),
}

pub struct Db {
    backend: Backend,
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
            backend: Self::create_backend(path)?,
        })
    }

    pub fn get_records_for_job_paginated(
        &self,
        job_id: &str,
        limit: usize,
        after_timestamp: Option<u64>,
    ) -> Result<Vec<Record>> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.get_records_for_job_paginated(job_id, limit, after_timestamp),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.get_records_for_job_paginated(job_id, limit, after_timestamp),
        }
    }

    pub fn get_all_records(
        &self,
        limit_per_job: Option<usize>,
    ) -> Result<HashMap<String, Vec<Record>>> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.get_all_records(limit_per_job),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.get_all_records(limit_per_job),
        }
    }

    pub fn batch_check_and_insert(
        &self,
        job: &JobConfig,
        items: Vec<ExtractedItem>,
    ) -> Result<Vec<ExtractedItem>> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.batch_check_and_insert(job, items),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.batch_check_and_insert(job, items),
        }
    }

    pub fn lua_set(&self, key: &str, value: &str) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_set(key, value),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_set(key, value),
        }
    }

    pub fn lua_get(&self, key: &str) -> Result<Option<String>> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_get(key),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_get(key),
        }
    }

    pub fn lua_delete(&self, key: &str) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_delete(key),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_delete(key),
        }
    }

    pub fn lua_incr(&self, key: &str, default: i64, delta: i64) -> Result<i64> {
        match &self.backend {
            #[cfg(feature = "redb")]
            Backend::Redb(b) => b.lua_incr(key, default, delta),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(b) => b.lua_incr(key, default, delta),
        }
    }

    #[cfg(feature = "sqlite")]
    pub fn db_query(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<HashMap<String, sqlite::SqlValue>>> {
        match &self.backend {
            Backend::Sqlite(b) => b.db_query(sql, params),
            #[cfg(feature = "redb")]
            Backend::Redb(_) => anyhow::bail!("db_query is only available in the sqlite backend"),
        }
    }

    #[cfg(feature = "sqlite")]
    pub fn db_exec(&self, sql: &str, params: &[rusqlite::types::Value]) -> Result<u64> {
        match &self.backend {
            Backend::Sqlite(b) => b.db_exec(sql, params),
            #[cfg(feature = "redb")]
            Backend::Redb(_) => anyhow::bail!("db_exec is only available in the sqlite backend"),
        }
    }
}
