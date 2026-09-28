use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, Once};
use std::time::Duration;

type ShutdownHook = Box<dyn FnOnce() + Send>;

pub enum Phase {
    Pre,
    Post,
}

static PRE_HOOKS: Mutex<Vec<ShutdownHook>> = Mutex::new(Vec::new());
static POST_HOOKS: Mutex<Vec<ShutdownHook>> = Mutex::new(Vec::new());
static SHUTDOWN_ONCE: Once = Once::new();

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

pub fn nanos_to_zulu(time: Option<u64>) -> String {
    use chrono::{DateTime, Utc};

    let dt = match time {
        Some(nanos) => DateTime::from_timestamp_nanos(nanos as i64),
        None => Utc::now(),
    };

    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn now_zulu() -> String {
    nanos_to_zulu(None)
}

pub fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{:.2}ms", duration.as_secs_f64() * 1000.0)
    }
}

pub fn register_shutdown_hook(phase: Phase, hook: impl FnOnce() + Send + 'static) {
    match phase {
        Phase::Pre => push_hook(&PRE_HOOKS, hook),
        Phase::Post => push_hook(&POST_HOOKS, hook),
    }
}

fn push_hook(target: &'static Mutex<Vec<ShutdownHook>>, hook: impl FnOnce() + Send + 'static) {
    match target.lock() {
        Ok(mut hooks) => hooks.push(Box::new(hook)),
        Err(poisoned) => poisoned.into_inner().push(Box::new(hook)),
    }
}

fn run_hooks(target: &'static Mutex<Vec<ShutdownHook>>) {
    let hooks = std::mem::take(&mut *target.lock().unwrap_or_else(|e| e.into_inner()));
    for hook in hooks.into_iter().rev() {
        if let Err(e) = catch_unwind(AssertUnwindSafe(hook)) {
            crate::t_eprintln!("shutdown hook panicked: {:?}", e);
        }
    }
}

pub fn shutdown_system() {
    SHUTDOWN_ONCE.call_once(|| {
        run_hooks(&PRE_HOOKS);

        if let Err(e) = catch_unwind(AssertUnwindSafe(|| {
            crate::cdp::browser::shutdown_all();
            crate::services::io::shutdown();
            std::thread::sleep(Duration::from_millis(500));
        })) {
            crate::t_eprintln!("shutdown teardown panicked: {:?}", e);
        }

        run_hooks(&POST_HOOKS);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_shutdown_hooks_phases_lifo_and_once() {
        let order = Arc::new(Mutex::new(Vec::new()));

        register_shutdown_hook(Phase::Pre, {
            let o = Arc::clone(&order);
            move || o.lock().unwrap().push("pre1")
        });
        register_shutdown_hook(Phase::Pre, || panic!("pre hook boom"));
        register_shutdown_hook(Phase::Pre, {
            let o = Arc::clone(&order);
            move || o.lock().unwrap().push("pre2")
        });
        register_shutdown_hook(Phase::Post, {
            let o = Arc::clone(&order);
            move || o.lock().unwrap().push("post1")
        });
        register_shutdown_hook(Phase::Post, {
            let o = Arc::clone(&order);
            move || o.lock().unwrap().push("post2")
        });

        shutdown_system();
        shutdown_system(); // must not re-run hooks

        assert_eq!(
            *order.lock().unwrap(),
            vec!["pre2", "pre1", "post2", "post1"]
        );
    }
}
