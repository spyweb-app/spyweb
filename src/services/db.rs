use crate::{config::types::JobConfig, scraper::extractor::ExtractedItem};
use anyhow::Result;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const RECORDS: TableDefinition<(&str, u64), &str> = TableDefinition::new("records");
const SEEN: TableDefinition<&str, u8> = TableDefinition::new("seen");
pub const LUA_USER_TABLE: TableDefinition<&str, &str> = TableDefinition::new("lua_user");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub fields: HashMap<String, String>,
    pub timestamp: u64,
    pub hash: String,
    pub match_key: Vec<String>,
}

impl Record {
    pub fn datetime_zulu(&self) -> String {
        crate::services::utils::nanos_to_zulu(Some(self.timestamp))
    }
}

pub struct Db {
    db: Database,
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let db = Database::create(path)?;
        let write = db.begin_write()?;
        write.open_table(RECORDS)?;
        write.open_table(SEEN)?;
        write.commit()?;
        Ok(Self { db })
    }

    pub fn begin_read(&self) -> Result<redb::ReadTransaction> {
        Ok(self.db.begin_read()?)
    }

    pub fn begin_write(&self) -> Result<redb::WriteTransaction> {
        Ok(self.db.begin_write()?)
    }

    // ── Records ──────────────────────────────────────────────────────────────

    pub fn insert_record1(
        &self,
        job: &JobConfig,
        fields: HashMap<String, String>,
        hash: &str,
    ) -> Result<()> {
        let timestamp = crate::services::utils::now_nanos();
        let reversed = u64::MAX - timestamp;
        let record = Record {
            fields,
            timestamp,
            hash: hash.to_string(),
            match_key: vec![],
        };
        let json = serde_json::to_string(&record)?;
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RECORDS)?;
            table.insert((job.id().as_str(), reversed), json.as_str())?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn get_records_for_job(&self, job: &JobConfig) -> Result<Vec<Record>> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RECORDS)?;
        let job_id = job.id();
        let range = (job_id.as_str(), 0)..=(job_id.as_str(), u64::MAX);

        let records = table
            .range(range)?
            .filter_map(|entry| {
                let (_, v) = entry.ok()?;
                serde_json::from_str(v.value()).ok()
            })
            .collect();

        Ok(records)
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

        for entry in table.iter()? {
            let (k, v) = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let (job_id, _) = k.value();

            if job_id != current_job {
                current_job = job_id.to_string();
                current_count = 0;
            }

            if current_count < max_limit
                && let Ok(record) = serde_json::from_str::<Record>(v.value())
            {
                grouped.entry(current_job.clone()).or_default().push(record);
                current_count += 1;
            }
        }

        Ok(grouped)
    }

    pub fn get_job_ids(&self) -> Result<Vec<String>> {
        let read = self.db.begin_read()?;
        let table = read.open_table(RECORDS)?;

        let mut job_ids = Vec::new();
        let mut current_job = String::new();

        for entry in table.iter()? {
            let (k, _) = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let (job_id, _) = k.value();

            if job_id != current_job {
                current_job = job_id.to_string();
                job_ids.push(current_job.clone());
            }
        }

        Ok(job_ids)
    }

    pub fn delete_record(&self, job: &JobConfig, timestamp: u64) -> Result<()> {
        let reversed = u64::MAX - timestamp;
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RECORDS)?;
            table.remove((job.id().as_str(), reversed))?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn clear_job_records(&self, job: &JobConfig) -> Result<()> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(RECORDS)?;
            let job_id = job.id();
            let range = (job_id.as_str(), 0)..=(job_id.as_str(), u64::MAX);

            let keys: Vec<(String, u64)> = table
                .range(range)?
                .filter_map(|e| {
                    let (k, _) = e.ok()?;
                    Some((k.value().0.to_string(), k.value().1))
                })
                .collect();

            for (jid, rev) in keys {
                table.remove((jid.as_str(), rev))?;
            }
        }
        write.commit()?;
        Ok(())
    }

    // ── Dedup ─────────────────────────────────────────────────────────────────

    pub fn is_seen(&self, hash: &str) -> Result<bool> {
        let read = self.db.begin_read()?;
        let table = read.open_table(SEEN)?;
        Ok(table.get(hash)?.is_some())
    }

    pub fn mark_seen(&self, hash: &str) -> Result<()> {
        let write = self.db.begin_write()?;
        {
            let mut table = write.open_table(SEEN)?;
            table.insert(hash, 1u8)?;
        }
        write.commit()?;
        Ok(())
    }

    pub fn is_seen_and_mark(&self, hash: &str) -> Result<bool> {
        let write = self.db.begin_write()?;
        let already_seen;
        {
            let mut table = write.open_table(SEEN)?;
            already_seen = table.get(hash)?.is_some();
            if !already_seen {
                table.insert(hash, 1u8)?;
            }
        }
        write.commit()?;
        Ok(already_seen)
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
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn normalized_hash_fields(job: &JobConfig) -> Vec<String> {
    job.hash_fields
        .as_ref()
        .map(|fields| {
            fields
                .iter()
                .map(|field| field.trim())
                .filter(|field| !field.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn hash_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut pairs: Vec<_> = pairs.into_iter().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);

    let mut buffer = Vec::with_capacity(pairs.len() * 64);

    for (k, v) in pairs {
        buffer.extend_from_slice(k.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(v.as_bytes());
        buffer.push(0);
    }

    let hash = xxhash_rust::xxh3::xxh3_64(&buffer);
    format!("{:016x}", hash)
}

pub fn hash_fields(job: &JobConfig, fields: &HashMap<String, String>) -> String {
    let configured = normalized_hash_fields(job);
    if configured.is_empty() {
        return hash_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let selected: Vec<(&str, &str)> = configured
        .iter()
        .map(|name| {
            let value = fields.get(name).map(String::as_str).unwrap_or("");
            (name.as_str(), value)
        })
        .collect();

    let empty_fields: Vec<&str> = selected
        .iter()
        .filter_map(|(name, value)| value.trim().is_empty().then_some(*name))
        .collect();

    if !empty_fields.is_empty() {
        crate::t_eprintln!(
            "Job '{}' has empty hash_fields at runtime: {}",
            job.name,
            empty_fields.join(", ")
        );
    }

    if empty_fields.len() == selected.len() {
        crate::t_eprintln!(
            "Job '{}' has all configured hash_fields empty at runtime; falling back to hashing all extracted fields",
            job.name
        );
        return hash_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    hash_pairs(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TestDb {
        path: String,
        db: Option<Db>,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = format!("{}.redb", name);
            if fs::metadata(&path).is_ok() {
                fs::remove_file(&path).unwrap();
            }
            let db = Db::open(&path).unwrap();
            Self { path, db: Some(db) }
        }

        fn db(&self) -> &Db {
            self.db.as_ref().unwrap()
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            self.db = None;
            let _ = fs::remove_file(&self.path);
        }
    }

    fn mock_job(id: &str) -> JobConfig {
        JobConfig {
            name: id.to_string(),
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
        }
    }

    #[test]
    fn test_db_operations() {
        let tdb = TestDb::new("test_ops");
        let job = mock_job("test-job");
        let mut fields = HashMap::new();
        fields.insert("title".to_string(), "Rust Job".to_string());

        // Insert
        tdb.db()
            .insert_record1(&job, fields.clone(), "hash1")
            .unwrap();

        // Get records
        let records = tdb.db().get_records_for_job(&job).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].fields, fields);
        assert_eq!(records[0].hash, "hash1");

        // All records
        let all = tdb.db().get_all_records(None).unwrap();
        assert_eq!(all.len(), 1);

        // Seen logic
        assert!(!tdb.db().is_seen("h1").unwrap());
        tdb.db().mark_seen("h1").unwrap();
        assert!(tdb.db().is_seen("h1").unwrap());

        assert!(!tdb.db().is_seen_and_mark("h2").unwrap());
        assert!(tdb.db().is_seen_and_mark("h2").unwrap());

        // Delete
        let ts = records[0].timestamp;
        tdb.db().delete_record(&job, ts).unwrap();
        assert!(tdb.db().get_records_for_job(&job).unwrap().is_empty());
    }

    #[test]
    fn test_batch_check_and_insert() {
        let tdb = TestDb::new("test_batch");
        let job = mock_job("batch-job");

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

        // First batch: both are new
        let new = tdb
            .db()
            .batch_check_and_insert(&job, vec![item1.clone(), item2.clone()])
            .unwrap();
        assert_eq!(new.len(), 2);

        // Second batch: one existing, one new
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
    fn test_hash_fields_consistency() {
        let mut fields = HashMap::new();
        fields.insert("a".to_string(), "1".to_string());
        fields.insert("b".to_string(), "2".to_string());

        let h1 = hash_fields(&mock_job("hash-job"), &fields);
        let h2 = hash_fields(&mock_job("hash-job"), &fields);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_fields_uses_configured_subset() {
        let mut job = mock_job("subset-job");
        job.hash_fields = Some(vec!["title".to_string()]);

        let mut fields1 = HashMap::new();
        fields1.insert("title".to_string(), "same".to_string());
        fields1.insert("date".to_string(), "10:00".to_string());

        let mut fields2 = HashMap::new();
        fields2.insert("title".to_string(), "same".to_string());
        fields2.insert("date".to_string(), "12:00".to_string());

        assert_eq!(hash_fields(&job, &fields1), hash_fields(&job, &fields2));
    }

    #[test]
    fn test_hash_fields_falls_back_to_all_fields_when_all_configured_values_are_empty() {
        let mut job = mock_job("fallback-job");
        job.hash_fields = Some(vec!["title".to_string(), "price".to_string()]);

        let mut fields1 = HashMap::new();
        fields1.insert("title".to_string(), "".to_string());
        fields1.insert("price".to_string(), " ".to_string());
        fields1.insert("date".to_string(), "10:00".to_string());

        let mut fields2 = HashMap::new();
        fields2.insert("title".to_string(), "".to_string());
        fields2.insert("price".to_string(), " ".to_string());
        fields2.insert("date".to_string(), "12:00".to_string());

        assert_ne!(hash_fields(&job, &fields1), hash_fields(&job, &fields2));
    }
}
