use crate::config::types::JobConfig;
use crate::scraper::extractor::ExtractedItem;
use crate::services::db::helpers::{Record, hash_fields};
use anyhow::Result;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::collections::HashMap;

const RECORDS: TableDefinition<(&str, u64), &str> = TableDefinition::new("records");
const SEEN: TableDefinition<&str, u8> = TableDefinition::new("seen");
const LUA_USER: TableDefinition<&str, &str> = TableDefinition::new("lua_user");

const DB_CACHE_SIZE_BYTES: usize = 128 * 1024;

pub struct RedbBackend {
    db: Database,
}

impl RedbBackend {
    pub fn open(path: &str) -> Result<Self> {
        let db = Database::builder()
            .set_cache_size(DB_CACHE_SIZE_BYTES)
            .create(path)?;
        let write = db.begin_write()?;
        write.open_table(RECORDS)?;
        write.open_table(SEEN)?;
        write.open_table(LUA_USER)?;
        write.commit()?;
        Ok(Self { db })
    }

    pub fn get_records_for_job_paginated(
        &self,
        job_id: &str,
        limit: usize,
        after_timestamp: Option<u64>,
    ) -> Result<Vec<Record>> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RECORDS)?;

        let mut records = Vec::with_capacity(limit.min(512));

        let start_reversed = match after_timestamp {
            Some(ts) => (u64::MAX - ts).saturating_add(1),
            None => 0,
        };

        let range = (job_id, start_reversed)..=(job_id, u64::MAX);

        for entry in table.range(range)? {
            let (_, v) = entry?;

            if let Ok(record) = serde_json::from_str(v.value()) {
                records.push(record);
            }

            if records.len() >= limit {
                break;
            }
        }

        Ok(records)
    }

    pub fn get_all_records(
        &self,
        limit_per_job: Option<usize>,
    ) -> Result<HashMap<String, Vec<Record>>> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RECORDS)?;

        let mut grouped: HashMap<String, Vec<Record>> = HashMap::new();
        let max_limit = limit_per_job.unwrap_or(usize::MAX);

        let mut current_job = String::new();
        let mut current_count = 0;

        for entry in table.iter()?.flatten() {
            let (job_id, _) = entry.0.value();

            if job_id != current_job {
                current_job = job_id.to_string();
                current_count = 0;
            }

            if current_count < max_limit
                && let Ok(record) = serde_json::from_str::<Record>(entry.1.value())
            {
                grouped.entry(current_job.clone()).or_default().push(record);
                current_count += 1;
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

        let write = self.db.begin_write()?;

        {
            let mut seen_table = write.open_table(SEEN)?;
            let mut records_table = write.open_table(RECORDS)?;

            for item in items {
                let hash = hash_fields(job, &item.fields);

                if seen_table.get(hash.as_str())?.is_some() {
                    continue;
                }

                seen_table.insert(hash.as_str(), 1u8)?;

                let timestamp = crate::services::utils::now_nanos();
                let reversed = u64::MAX - timestamp;

                let record = Record {
                    fields: item.fields.clone(),
                    timestamp,
                    hash: hash.clone(),
                    match_key: item.matches.clone(),
                };

                let json = serde_json::to_string(&record)?;
                records_table.insert((job.id().as_str(), reversed), json.as_str())?;

                new_items.push(item);
            }
        }

        write.commit()?;

        Ok(new_items)
    }

    pub fn lua_set(&self, key: &str, value: &str) -> Result<()> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(LUA_USER)?;
            table.insert(key, value)?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn lua_get(&self, key: &str) -> Result<Option<String>> {
        let read = self.db.begin_read()?;
        let table = read.open_table(LUA_USER)?;
        match table.get(key)? {
            Some(v) => Ok(Some(v.value().to_string())),
            None => Ok(None),
        }
    }

    pub fn lua_delete(&self, key: &str) -> Result<()> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(LUA_USER)?;
            table.remove(key)?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn lua_incr(&self, key: &str, default: i64, delta: i64) -> Result<i64> {
        let write = self.db.begin_write()?;
        let result = {
            let mut table = write.open_table(LUA_USER)?;
            let current = table
                .get(key)?
                .and_then(|v| v.value().parse::<i64>().ok())
                .unwrap_or(default);
            let new_val = current + delta;
            table.insert(key, new_val.to_string().as_str())?;
            new_val
        };
        write.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TestDb {
        path: String,
        db: Option<RedbBackend>,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = format!("{}.redb", name);
            if fs::metadata(&path).is_ok() {
                fs::remove_file(&path).unwrap();
            }
            let db = RedbBackend::open(&path).unwrap();
            Self { path, db: Some(db) }
        }

        fn db(&self) -> &RedbBackend {
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
        let tdb = TestDb::new("redb_test_batch");
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
    fn test_pagination() {
        let tdb = TestDb::new("redb_test_pagination");
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
        let tdb = TestDb::new("redb_test_all_records");

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
