use crate::config::types::JobConfig;
use crate::scraper::extractor::ExtractedItem;
use crate::services::db::helpers::{Record, hash_fields};
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

pub struct SqliteBackend {
    conn: Mutex<Connection>,
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS records (
            job_id    TEXT    NOT NULL,
            rev_ts    INTEGER NOT NULL,
            json      TEXT    NOT NULL,
            PRIMARY KEY (job_id, rev_ts)
        );
        CREATE TABLE IF NOT EXISTS seen (
            hash TEXT PRIMARY KEY
        );
        CREATE TABLE IF NOT EXISTS lua_user (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

impl SqliteBackend {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        create_tables(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_records_for_job_paginated(
        &self,
        job_id: &str,
        limit: usize,
        after_timestamp: Option<u64>,
    ) -> Result<Vec<Record>> {
        let conn = self.conn.lock().unwrap();

        let mut records = Vec::with_capacity(limit.min(512));

        let mut stmt = if after_timestamp.is_some() {
            conn.prepare(
                "SELECT json FROM records WHERE job_id = ?1 AND rev_ts < ?2 ORDER BY rev_ts DESC LIMIT ?3",
            )?
        } else {
            conn.prepare(
                "SELECT json FROM records WHERE job_id = ?1 ORDER BY rev_ts DESC LIMIT ?2",
            )?
        };

        let rows: Vec<String> = if let Some(after) = after_timestamp {
            stmt.query_map(rusqlite::params![job_id, after, limit as i64], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })?
            .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(rusqlite::params![job_id, limit as i64], |row| {
                let json: String = row.get(0)?;
                Ok(json)
            })?
            .collect::<Result<Vec<_>, _>>()?
        };

        for json in rows {
            if let Ok(record) = serde_json::from_str::<Record>(&json) {
                records.push(record);
            }
        }

        Ok(records)
    }

    pub fn get_all_records(
        &self,
        limit_per_job: Option<usize>,
    ) -> Result<HashMap<String, Vec<Record>>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit_per_job.unwrap_or(i64::MAX as usize);

        let mut stmt = conn.prepare(
            "SELECT job_id, json FROM (
                SELECT job_id, json, row_number() OVER (PARTITION BY job_id ORDER BY rev_ts DESC) AS rn
                FROM records
            ) WHERE rn <= ?1",
        )?;

        let rows = stmt.query_map([limit as i64], |row| {
            let job_id: String = row.get(0)?;
            let json: String = row.get(1)?;
            Ok((job_id, json))
        })?;

        let mut grouped: HashMap<String, Vec<Record>> = HashMap::new();
        for row in rows {
            if let Ok((job_id, json)) = row {
                if let Ok(record) = serde_json::from_str::<Record>(&json) {
                    grouped.entry(job_id).or_default().push(record);
                }
            }
        }

        Ok(grouped)
    }

    pub fn batch_check_and_insert(
        &self,
        job: &JobConfig,
        items: Vec<ExtractedItem>,
    ) -> Result<Vec<ExtractedItem>> {
        let mut new_items = Vec::with_capacity(items.len());
        let mut conn = self.conn.lock().unwrap();

        let tx = conn.transaction()?;

        let mut seen_stmt = tx.prepare("INSERT OR IGNORE INTO seen (hash) VALUES (?1)")?;
        let mut insert_stmt =
            tx.prepare("INSERT INTO records (job_id, rev_ts, json) VALUES (?1, ?2, ?3)")?;

        for item in &items {
            let hash = hash_fields(job, &item.fields);

            seen_stmt.execute(rusqlite::params![&hash])?;

            if tx.changes() == 0 {
                continue;
            }

            let timestamp = crate::services::utils::now_nanos();

            let record = Record {
                fields: item.fields.clone(),
                timestamp,
                hash: hash.clone(),
                match_key: item.matches.clone(),
            };

            let json = serde_json::to_string(&record)?;
            insert_stmt.execute(rusqlite::params![job.id().as_str(), timestamp, json])?;
            new_items.push(item.clone());
        }

        drop(seen_stmt);
        drop(insert_stmt);
        tx.commit()?;

        Ok(new_items)
    }

    pub fn lua_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO lua_user (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    pub fn lua_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM lua_user WHERE key = ?1")?;
        let mut rows = stmt.query_map(rusqlite::params![key], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        })?;
        match rows.next() {
            Some(Ok(value)) => Ok(Some(value)),
            _ => Ok(None),
        }
    }

    pub fn lua_delete(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM lua_user WHERE key = ?1",
            rusqlite::params![key],
        )?;
        Ok(())
    }

    pub fn lua_incr(&self, key: &str, default: i64, delta: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "INSERT INTO lua_user (key, value) VALUES (?1, CAST(?2 + ?3 AS TEXT))
                ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + ?3 AS TEXT)
                RETURNING CAST(value AS INTEGER)",
        )?;
        let result: i64 =
            stmt.query_row(rusqlite::params![key, default, delta], |row| row.get(0))?;
        Ok(result)
    }

    pub fn db_query(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> anyhow::Result<Vec<HashMap<String, SqlValue>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let mut map = HashMap::new();
            for (i, col) in cols.iter().enumerate() {
                let val = match row.get::<_, rusqlite::types::Value>(i) {
                    Ok(v) => match v {
                        rusqlite::types::Value::Null => SqlValue::Null,
                        rusqlite::types::Value::Integer(n) => SqlValue::Integer(n),
                        rusqlite::types::Value::Real(f) => SqlValue::Real(f),
                        rusqlite::types::Value::Text(s) => SqlValue::Text(s),
                        rusqlite::types::Value::Blob(b) => {
                            SqlValue::Text(String::from_utf8_lossy(&b).to_string())
                        }
                    },
                    Err(_) => SqlValue::Null,
                };
                map.insert(col.clone(), val);
            }
            Ok(map)
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn db_exec(&self, sql: &str, params: &[rusqlite::types::Value]) -> anyhow::Result<u64> {
        let conn = self.conn.lock().unwrap();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let changes = conn.execute(sql, param_refs.as_slice())?;
        Ok(changes as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TestDb {
        path: String,
        db: Option<SqliteBackend>,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = format!("{}.sqlite", name);
            if fs::metadata(&path).is_ok() {
                fs::remove_file(&path).unwrap();
            }
            let db = SqliteBackend::open(&path).unwrap();
            Self { path, db: Some(db) }
        }

        fn db(&self) -> &SqliteBackend {
            self.db.as_ref().unwrap()
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            self.db = None;
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn test_batch_check_and_insert() {
        let tdb = TestDb::new("sqlite_test_batch");
        let job = JobConfig {
            name: "batch-job".to_string(),
            url: "https://example.com".to_string(),
            selector: ".item".to_string(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            debug: false,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
        };

        let item1 = ExtractedItem {
            fields: [("id".to_string(), "1".to_string())].into_iter().collect(),
            parent_html: None,
            field_match_html: None,
            matches: vec![],
            ..Default::default()
        };
        let item2 = ExtractedItem {
            fields: [("id".to_string(), "2".to_string())].into_iter().collect(),
            parent_html: None,
            field_match_html: None,
            matches: vec![],
            ..Default::default()
        };

        let new = tdb
            .db()
            .batch_check_and_insert(&job, vec![item1.clone(), item2.clone()])
            .unwrap();
        assert_eq!(new.len(), 2);

        let item3 = ExtractedItem {
            fields: [("id".to_string(), "3".to_string())].into_iter().collect(),
            parent_html: None,
            field_match_html: None,
            matches: vec![],
            ..Default::default()
        };
        let new = tdb
            .db()
            .batch_check_and_insert(&job, vec![item1, item3])
            .unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].fields.get("id").unwrap(), "3");
    }

    #[test]
    fn test_lua_storage() {
        let tdb = TestDb::new("sqlite_test_lua");
        let db = tdb.db();

        db.lua_set("greeting", "hello").unwrap();
        assert_eq!(db.lua_get("greeting").unwrap(), Some("hello".to_string()));

        assert_eq!(db.lua_get("nonexistent").unwrap(), None);

        db.lua_set("greeting", "world").unwrap();
        assert_eq!(db.lua_get("greeting").unwrap(), Some("world".to_string()));

        db.lua_delete("greeting").unwrap();
        assert_eq!(db.lua_get("greeting").unwrap(), None);

        let val = db.lua_incr("counter", 0, 1).unwrap();
        assert_eq!(val, 1);

        let val = db.lua_incr("counter", 0, 5).unwrap();
        assert_eq!(val, 6);

        let val = db.lua_incr("counter", 100, 1).unwrap();
        assert_eq!(val, 7);
    }

    #[test]
    fn test_pagination() {
        let tdb = TestDb::new("sqlite_test_pagination");
        let job = JobConfig {
            name: "pagination-job".to_string(),
            url: "https://example.com".to_string(),
            selector: ".item".to_string(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            debug: false,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
        };

        let items: Vec<ExtractedItem> = (0..10)
            .map(|i| ExtractedItem {
                fields: [("n".to_string(), i.to_string())].into_iter().collect(),
                parent_html: None,
                field_match_html: None,
                matches: vec![],
                ..Default::default()
            })
            .collect();

        let job_id = job.id();
        let new = tdb.db().batch_check_and_insert(&job, items).unwrap();
        assert_eq!(new.len(), 10);

        let page1 = tdb
            .db()
            .get_records_for_job_paginated(&job_id, 3, None)
            .unwrap();
        assert_eq!(page1.len(), 3);

        // Descending order (newest first)
        assert!(page1[0].timestamp > page1[1].timestamp);
        assert!(page1[1].timestamp > page1[2].timestamp);

        // Second page — cursor is last record's timestamp
        let cursor = page1.last().unwrap().timestamp;
        let page2 = tdb
            .db()
            .get_records_for_job_paginated(&job_id, 3, Some(cursor))
            .unwrap();
        assert_eq!(page2.len(), 3);
        for rec in &page2 {
            assert!(rec.timestamp < cursor);
        }

        // Third page
        let cursor2 = page2.last().unwrap().timestamp;
        let page3 = tdb
            .db()
            .get_records_for_job_paginated(&job_id, 3, Some(cursor2))
            .unwrap();
        assert_eq!(page3.len(), 3);
        for rec in &page3 {
            assert!(rec.timestamp < cursor2);
        }

        // Fourth page — single remaining record
        let cursor3 = page3.last().unwrap().timestamp;
        let page4 = tdb
            .db()
            .get_records_for_job_paginated(&job_id, 3, Some(cursor3))
            .unwrap();
        assert_eq!(page4.len(), 1);
        assert!(page4[0].timestamp < cursor3);
    }

    #[test]
    fn test_get_all_records() {
        let tdb = TestDb::new("sqlite_test_all_records");

        let job_a = JobConfig {
            name: "all-job-a".to_string(),
            url: "https://example.com".to_string(),
            selector: ".item".to_string(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            debug: false,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
        };
        let job_b = JobConfig {
            name: "all-job-b".to_string(),
            url: "https://example.com".to_string(),
            selector: ".item".to_string(),
            fields: vec![],
            keywords: None,
            search_fields: None,
            webhook: None,
            enabled: true,
            interval: 60,
            debug: false,
            proxy: None,
            notification: None,
            headers: None,
            hash_fields: None,
            workers: None,
            urls: None,
        };

        let items_a: Vec<ExtractedItem> = (0..3)
            .map(|i| ExtractedItem {
                fields: [("k".to_string(), format!("a{}", i))].into_iter().collect(),
                parent_html: None,
                field_match_html: None,
                matches: vec![],
                ..Default::default()
            })
            .collect();
        let items_b: Vec<ExtractedItem> = (0..5)
            .map(|i| ExtractedItem {
                fields: [("k".to_string(), format!("b{}", i))].into_iter().collect(),
                parent_html: None,
                field_match_html: None,
                matches: vec![],
                ..Default::default()
            })
            .collect();

        let id_a = job_a.id();
        let id_b = job_b.id();

        tdb.db().batch_check_and_insert(&job_a, items_a).unwrap();
        tdb.db().batch_check_and_insert(&job_b, items_b).unwrap();

        // All records, no limit
        let all = tdb.db().get_all_records(None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all.get(&id_a).unwrap().len(), 3);
        assert_eq!(all.get(&id_b).unwrap().len(), 5);

        // Respects per-job limit
        let limited = tdb.db().get_all_records(Some(2)).unwrap();
        assert_eq!(limited.get(&id_a).unwrap().len(), 2);
        assert_eq!(limited.get(&id_b).unwrap().len(), 2);

        // Newest-first ordering within each job
        let a_recs = limited.get(&id_a).unwrap();
        assert!(a_recs[0].timestamp > a_recs[1].timestamp);
    }
}
