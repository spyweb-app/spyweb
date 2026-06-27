use std::fs;
use std::sync::Arc;

use crate::config::types::{Field, Job, JobConfig};
use crate::config::{loader, validate};

fn mock_job(hash_fields: Option<Vec<&str>>) -> JobConfig {
    JobConfig {
        name: "test-job".to_string(),
        url: "https://example.com".to_string(),
        selector: ".item".to_string(),
        fields: vec![
            Field::Full {
                name: "title".to_string(),
                selector: ".title".to_string(),
                att: "text".to_string(),
            },
            Field::Shorthand("price:.price".to_string()),
            Field::Shorthand("date".to_string()),
        ],
        keywords: None,
        search_fields: None,
        webhook: None,
        debug: false,
        enabled: true,
        interval: 60,
        proxy: None,
        notification: None,
        headers: None,
        hash_fields: hash_fields.map(|fields| fields.into_iter().map(ToOwned::to_owned).collect()),
        workers: None,
        urls: None,
    }
}

fn wrap_job(config: JobConfig) -> Job {
    Job {
        config,
        hooks: None,
        has_hooks_file: false,
        dir: None,
    }
}

#[test]
fn validates_known_hash_fields() {
    let job = mock_job(Some(vec!["title", "price"]));
    assert!(validate::validate_job_config(&job).is_ok());
}

#[test]
fn ignores_blank_hash_fields() {
    let job = mock_job(Some(vec!["title", " ", ""]));
    assert!(validate::validate_job_config(&job).is_ok());
}

#[test]
fn rejects_unknown_hash_fields() {
    let job = mock_job(Some(vec!["title", "stupid_typo"]));
    let err = validate::validate_job_config(&job).unwrap_err().to_string();
    assert!(err.contains("Invalid hash_fields for job 'test-job'"));
    assert!(err.contains("stupid_typo"));
    assert!(err.contains("title, price, date"));
}

#[test]
fn rejects_blank_required_fields() {
    let mut job = mock_job(None);
    job.name = " ".to_string();
    assert_eq!(
        validate::validate_job_config(&job).unwrap_err().to_string(),
        "Invalid job config: name cannot be blank"
    );

    let mut job = mock_job(None);
    job.url = " ".to_string();
    assert_eq!(
        validate::validate_job_config(&job).unwrap_err().to_string(),
        "Invalid job config for 'test-job': url cannot be blank"
    );

    let mut job = mock_job(None);
    job.selector = " ".to_string();
    assert_eq!(
        validate::validate_job_config(&job).unwrap_err().to_string(),
        "Invalid job config for 'test-job': selector cannot be blank"
    );
}

#[test]
fn rejects_empty_fields_and_zero_interval() {
    let mut job = mock_job(None);
    job.fields = vec![];
    assert_eq!(
        validate::validate_job_config(&job).unwrap_err().to_string(),
        "Invalid job config for 'test-job': fields cannot be empty"
    );

    let mut job = mock_job(None);
    job.interval = 0;
    assert_eq!(
        validate::validate_job_config(&job).unwrap_err().to_string(),
        "Invalid job config for 'test-job': interval must be > 0"
    );
}

#[test]
fn rejects_invalid_urls() {
    let mut job = mock_job(None);
    job.url = "not-a-url".to_string();
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("Invalid url for job 'test-job'")
    );

    let mut job = mock_job(None);
    job.webhook = Some(crate::config::types::Webhook {
        enabled: true,
        url: "bad-webhook".to_string(),
        headers: None,
    });
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("Invalid webhook.url for job 'test-job'")
    );

    let mut job = mock_job(None);
    job.proxy = Some(crate::config::types::Proxy {
        enabled: true,
        rotate: crate::config::types::Rotate::Random,
        urls: vec!["bad-proxy".to_string()],
    });
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("Invalid proxy.urls for job 'test-job'")
    );
}

#[test]
fn rejects_invalid_field_definitions_and_search_fields() {
    let mut job = mock_job(None);
    job.fields = vec![Field::Shorthand("   ".to_string())];
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("shorthand field cannot be blank")
    );

    let mut job = mock_job(None);
    job.fields = vec![Field::Full {
        name: "title".to_string(),
        selector: "   ".to_string(),
        att: "text".to_string(),
    }];
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("selector cannot be blank")
    );

    let mut job = mock_job(None);
    job.fields = vec![
        Field::Shorthand("title:.title".to_string()),
        Field::Full {
            name: "title".to_string(),
            selector: ".title-2".to_string(),
            att: "text".to_string(),
        },
    ];
    assert!(
        validate::validate_job_config(&job)
            .unwrap_err()
            .to_string()
            .contains("duplicate field name(s): title")
    );

    let mut job = mock_job(None);
    job.search_fields = Some(vec!["title".to_string(), "missing".to_string()]);
    let err = validate::validate_job_config(&job).unwrap_err().to_string();
    assert!(err.contains("Invalid search_fields for job 'test-job'"));
    assert!(err.contains("missing"));
}

#[test]
fn rejects_duplicate_job_names_and_ids() {
    let err = validate::validate_job_set(&[wrap_job(mock_job(None)), wrap_job(mock_job(None))])
        .unwrap_err()
        .to_string();
    assert!(err.contains("duplicate job name(s): test-job"));

    let mut first = mock_job(None);
    first.name = "Hello World".to_string();
    let mut second = mock_job(None);
    second.name = "hello-world".to_string();

    let err = validate::validate_job_set(&[wrap_job(first), wrap_job(second)])
        .unwrap_err()
        .to_string();
    assert!(err.contains("duplicate job id(s): hello_world"));
}

#[test]
fn skips_hook_loading_for_disabled_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let job_dir = dir.path().join("disabled-job");
    fs::create_dir_all(&job_dir).unwrap();

    fs::write(
        job_dir.join("config.toml"),
        r#"
name = "Disabled Job"
url = "https://example.com"
selector = ".item"
enabled = false
fields = ["title:.title"]
"#,
    )
    .unwrap();

    fs::write(job_dir.join("hooks.lua"), "this is not valid lua").unwrap();

    let db_path = dir.path().join("test.redb");
    let db = Arc::new(crate::services::db::Db::open(db_path.to_str().unwrap()).unwrap());
    let jobs = loader::load_dir_jobs(dir.path().to_str().unwrap(), db).unwrap();

    assert_eq!(jobs.len(), 1);
    assert!(!jobs[0].config.enabled);
    assert!(jobs[0].hooks.is_none());
    assert!(jobs[0].has_hooks_file);
}
