use super::JobHooks;
use crate::cdp::browser;
use anyhow::Result;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TELEMETRY_STAGES: [(&str, &str); 14] = [
    ("before_fetch", "hook"),
    ("override_fetch", "hook"),
    ("fetch", "internal"),
    ("after_fetch", "hook"),
    ("override_extract", "hook"),
    ("extract", "internal"),
    ("after_extract", "hook"),
    ("filter", "internal"),
    ("before_store", "hook"),
    ("store", "internal"),
    ("before_notify", "hook"),
    ("notify", "internal"),
    ("before_webhook", "hook"),
    ("webhook", "internal"),
];

pub(crate) struct TelemetrySample {
    pub(crate) started: Instant,
    pub(crate) offset_ms: f64,
    pub(crate) mem_before: usize,
}

impl JobHooks {
    pub(crate) fn render_and_log_hook_error(
        &self,
        hook_name: &'static str,
        err: anyhow::Error,
    ) -> String {
        let rendered = self.format_hook_error(hook_name, err);
        crate::t_eprintln!("{rendered}, skipping hook");
        rendered
    }

    pub(crate) async fn init_telemetry(&self) -> Result<()> {
        let lua = self.lua.lock().await;
        let telemetry = lua.create_table()?;
        let stages = lua.create_table()?;
        let map = lua.create_table()?;
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        telemetry.set("job_name", self.job_name.clone())?;
        telemetry.set("start_time", start_time)?;
        telemetry.set("total_duration_ms", 0.0)?;

        for (idx, (name, stage_type)) in TELEMETRY_STAGES.iter().enumerate() {
            let entry = lua.create_table()?;
            entry.set("name", *name)?;
            entry.set("status", "inactive")?;
            entry.set("type", *stage_type)?;
            stages.raw_set(idx + 1, entry.clone())?;
            map.set(*name, entry)?;
        }

        telemetry.set("stages", stages)?;
        telemetry.set("map", map)?;
        lua.globals().set("spyweb_telemetry", telemetry)?;
        let _ = lua.globals().set("__spyweb_filter_error", mlua::Value::Nil);
        Ok(())
    }

    pub(crate) async fn telemetry_stage_start(&self) -> Result<TelemetrySample> {
        let lua = self.lua.lock().await;
        let offset_ms = lua
            .globals()
            .get::<mlua::Table>("spyweb_telemetry")
            .and_then(|telemetry| telemetry.get::<f64>("start_time"))
            .map(|start_time| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
                (now - start_time) * 1000.0
            })
            .unwrap_or(0.0);

        Ok(TelemetrySample {
            started: Instant::now(),
            offset_ms,
            mem_before: lua.used_memory(),
        })
    }

    pub(crate) async fn record_telemetry_stage(
        &self,
        name: &str,
        sample: TelemetrySample,
        status: &str,
        error: Option<String>,
    ) -> Result<()> {
        let lua = self.lua.lock().await;
        let Ok(telemetry) = lua.globals().get::<mlua::Table>("spyweb_telemetry") else {
            return Ok(());
        };
        let map: mlua::Table = telemetry.get("map")?;
        let entry: mlua::Table = map.get(name)?;
        let mem_after = lua.used_memory();

        entry.set("status", status)?;
        entry.set("offset_ms", sample.offset_ms)?;
        entry.set(
            "duration_ms",
            sample.started.elapsed().as_secs_f64() * 1000.0,
        )?;
        entry.set(
            "mem_delta_bytes",
            mem_after as i64 - sample.mem_before as i64,
        )?;
        entry.set("lua_mem_bytes", mem_after)?;
        entry.set("browsers", browser::active_count())?;
        match error {
            Some(error) => entry.set("error", error)?,
            None => entry.set("error", mlua::Value::Nil)?,
        }

        Ok(())
    }

    pub(crate) async fn finalize_telemetry(&self, duration: Duration) -> Result<()> {
        let lua = self.lua.lock().await;
        if let Ok(telemetry) = lua.globals().get::<mlua::Table>("spyweb_telemetry") {
            telemetry.set("total_duration_ms", duration.as_secs_f64() * 1000.0)?;
        }
        Ok(())
    }

    pub(crate) async fn remember_filter_error(&self, error: String) {
        let lua = self.lua.lock().await;
        if lua
            .globals()
            .get::<Option<String>>("__spyweb_filter_error")
            .ok()
            .flatten()
            .is_none()
        {
            let _ = lua.globals().set("__spyweb_filter_error", error);
        }
    }

    pub(crate) async fn take_filter_error(&self) -> Option<String> {
        let lua = self.lua.lock().await;
        let error = lua
            .globals()
            .get::<Option<String>>("__spyweb_filter_error")
            .ok()
            .flatten();
        let _ = lua.globals().set("__spyweb_filter_error", mlua::Value::Nil);
        error
    }

    pub async fn print_telemetry(&self) {
        let lua = self.lua.lock().await;
        let Ok(telemetry) = lua.globals().get::<mlua::Table>("spyweb_telemetry") else {
            return;
        };

        println!(
            "\n{}",
            crate::color::c_bold("📊 Pipeline Telemetry Summary")
        );
        println!(
            "{}",
            crate::color::c_dim(
                "--------------------------------------------------------------------------------"
            )
        );

        let header = format!(
            "{} | {} | {} | {} | {}",
            crate::color::c_bold(&format!("{:<16}", "Stage")),
            crate::color::c_bold(&format!("{:<8}", "Status")),
            crate::color::c_bold(&format!("{:>11}", "Duration")),
            crate::color::c_bold(&format!("{:>20}", "Memory (Delta)")),
            crate::color::c_bold(&format!("{:>8}", "Browsers"))
        );
        println!("{}", header);
        println!(
            "{}",
            crate::color::c_dim(
                "--------------------------------------------------------------------------------"
            )
        );

        let stages: mlua::Table = match telemetry.get("stages") {
            Ok(s) => s,
            Err(_) => return,
        };

        let len = stages.raw_len();
        for i in 1..=len {
            if let Ok(entry) = stages.get::<mlua::Table>(i) {
                let name = entry.get::<String>("name").unwrap_or_default();
                let status = entry
                    .get::<String>("status")
                    .unwrap_or_else(|_| "inactive".to_string());

                let name_padded = format!("{:<16}", name);
                let name_col = match entry.get::<String>("type").unwrap_or_default().as_str() {
                    "hook" => crate::color::c_job(&name_padded),
                    _ => crate::color::c_info(&name_padded),
                };

                let status_col = match status.as_str() {
                    "success" => crate::color::c_ok("[  OK  ]"),
                    "error" => crate::color::c_err("[ FAIL ]"),
                    "inactive" => crate::color::c_dim("[ SKIP ]"),
                    _ => crate::color::c_warn(&format!("[ {:^4} ]", status)),
                };

                let duration_ms = entry.get::<f64>("duration_ms").unwrap_or(0.0);
                let duration_padded = if duration_ms > 0.0 {
                    format!("{:>8.1} ms", duration_ms)
                } else {
                    format!("{:>11}", "-")
                };
                let duration_col = if status == "inactive" || duration_ms == 0.0 {
                    crate::color::c_dim(&duration_padded)
                } else if duration_ms > 5000.0 {
                    crate::color::c_err(&duration_padded)
                } else if duration_ms > 1000.0 {
                    crate::color::c_warn(&duration_padded)
                } else {
                    duration_padded.to_string()
                };

                let lua_mem = entry.get::<i64>("lua_mem_bytes").unwrap_or(0) as f64;
                let mem_delta = entry.get::<i64>("mem_delta_bytes").unwrap_or(0) as f64;
                let mem_padded = if lua_mem > 0.0 {
                    let sign = if mem_delta > 0.0 { "+" } else { "" };
                    let formatted = format!(
                        "{:.1} KB ({}{:.1} KB)",
                        lua_mem / 1024.0,
                        sign,
                        mem_delta / 1024.0
                    );
                    format!("{:>20}", formatted)
                } else {
                    format!("{:>20}", "-")
                };
                let mem_col = if status == "inactive" || lua_mem == 0.0 {
                    crate::color::c_dim(&mem_padded)
                } else {
                    mem_padded.to_string()
                };

                let browsers = entry.get::<i64>("browsers").unwrap_or(0);
                let browsers_padded = if browsers > 0 {
                    format!("{:>8}", browsers)
                } else {
                    format!("{:>8}", "-")
                };
                let browsers_col = if status == "inactive" || browsers == 0 {
                    crate::color::c_dim(&browsers_padded)
                } else {
                    crate::color::c_info(&browsers_padded)
                };

                println!(
                    "{} | {} | {} | {} | {}",
                    name_col, status_col, duration_col, mem_col, browsers_col
                );
            }
        }

        println!(
            "{}",
            crate::color::c_dim(
                "--------------------------------------------------------------------------------"
            )
        );
        if let Ok(total) = telemetry.get::<f64>("total_duration_ms") {
            println!(
                "{:<27} {}",
                crate::color::c_bold("Total Duration:"),
                crate::color::c_info(&format!("{:.1} ms", total))
            );
        }
        println!();
    }
}
