//! Rolling, bounded one-second observations, not a complete scheduler trace.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
const MAX_FILE: u64 = 1024 * 1024;
fn directory() -> PathBuf {
    crate::settings::directory().join("activity-log")
}
fn file(dir: &Path, core: usize, minute: u64) -> PathBuf {
    dir.join(format!("cpu-{core}-{minute}.jsonl"))
}
fn error() -> &'static Mutex<Option<String>> {
    static E: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(None))
}
pub fn status() -> Value {
    json!({"error":*error().lock().unwrap(),"directory":directory()})
}
pub fn record(snapshot: &Value, cores: &[usize], telemetry: &Value) {
    let time = crate::now();
    let result = (|| -> Result<(), String> {
        let dir = directory();
        if !dir.exists() && cores.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Only our numeric minute files are eligible for retention cleanup.
        for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some((_, minute)) = parse_name(&name) {
                if minute < time.saturating_sub(3600) / 60 {
                    std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
                }
            }
        }
        for &core in cores {
            let a = &snapshot["activity"];
            let current = a["cores"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["id"] == core));
            let rows = current.and_then(|c| c["rows"].as_array());
            let compact:Vec<_>=rows.into_iter().flatten().take(256).map(|r|json!({"pid":r["pid"],"identity":r["identity"],"name":r["name"],"ms":r["ms"],"usage":r["usage"]})).collect();
            let interrupts = a["interrupts"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["id"] == core));
            let cpu = snapshot["cpus"]
                .as_array()
                .and_then(|cs| cs.iter().find(|c| c["id"] == core));
            let temperatures: Vec<f64> = telemetry["sensors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| {
                    s["hardware_type"] == "Cpu"
                        && s["kind"] == "Temperature"
                        && s["sample_time"]
                            .as_u64()
                            .is_some_and(|t| time.saturating_sub(t) < 20)
                        && !s["name"]
                            .as_str()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains("distance")
                })
                .filter_map(|s| s["value"].as_f64())
                .filter(|v| v.is_finite())
                .collect();
            let temperature = temperatures.into_iter().reduce(f64::max);
            let v = json!({"time":time,"core":core,"status":a["status"],"lag_ms":a["lag_ms"],"event_ticks":a["event_ticks"],"lost":a["lost"],"cpu_usage":cpu.map(|c|&c["usage"]),"system_cpu":snapshot["cpu_usage"],"cpu_temperature_max":temperature,"rows":compact,"truncated":rows.is_some_and(|r|r.len()>256),"interrupts":interrupts,"wakeup":crate::latency::latest()});
            let path = file(&dir, core, time / 60);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_FILE) {
                return Err("Activity log minute reached its 1 MiB limit".into());
            }
            let mut output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| e.to_string())?;
            writeln!(output, "{}", v).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    *error().lock().unwrap() = result.err();
}
fn parse_name(name: &str) -> Option<(usize, u64)> {
    let (c, m) = name
        .strip_prefix("cpu-")?
        .strip_suffix(".jsonl")?
        .split_once('-')?;
    Some((c.parse().ok()?, m.parse().ok()?))
}
pub fn read(core: usize, minutes: u64) -> Result<Value, String> {
    read_at(&directory(), core, minutes, crate::now())
}
fn read_at(dir: &Path, core: usize, minutes: u64, time: u64) -> Result<Value, String> {
    if core >= usize::BITS as usize || !(1..=60).contains(&minutes) {
        return Err("Invalid CPU or history interval".into());
    }
    let from = time.saturating_sub(minutes * 60);
    let mut samples = Vec::new();
    let mut damaged = 0;
    for minute in from / 60..=time / 60 {
        let path = file(dir, core, minute);
        let input = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        use std::io::Read;
        for line in BufReader::new(input.take(MAX_FILE + 65536))
            .lines()
            .take(300)
        {
            match line
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            {
                Some(v) if v["time"].as_u64().is_some_and(|t| t >= from && t <= time) => {
                    samples.push(v)
                }
                Some(_) => {}
                None => damaged += 1,
            }
        }
    }
    samples.sort_by_key(|v| v["time"].as_u64().unwrap_or(0));
    let mut rows: HashMap<String, Value> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for s in &samples {
        if s["status"] != "running" {
            continue;
        }
        let stamp = s["event_ticks"].as_i64().unwrap_or(0);
        if stamp != 0 && !seen.insert(stamp) {
            continue;
        }
        for r in s["rows"].as_array().into_iter().flatten() {
            let key = format!("{}:{}:{}", r["pid"], r["identity"], r["name"]);
            let row=rows.entry(key).or_insert_with(||json!({"pid":r["pid"],"identity":r["identity"],"name":r["name"],"ms":0.0,"first":s["time"],"last":s["time"],"samples":0}));
            row["ms"] = json!(row["ms"].as_f64().unwrap_or(0.0) + r["ms"].as_f64().unwrap_or(0.0));
            row["samples"] = json!(row["samples"].as_u64().unwrap_or(0) + 1);
            row["last"] = s["time"].clone();
        }
    }
    let mut rows: Vec<_> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        b["ms"]
            .as_f64()
            .unwrap()
            .total_cmp(&a["ms"].as_f64().unwrap())
    });
    Ok(
        json!({"core":core,"from":from,"to":time,"rows":rows,"samples":samples,"damaged_lines":damaged}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_preserves_identity_and_ignores_expired_or_duplicate_samples() {
        let dir = std::env::temp_dir().join(format!("corescope-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let values = [
            json!({"time":7000,"status":"running","event_ticks":1,"rows":[{"pid":9,"identity":"a","name":"old.exe","ms":50}]}),
            json!({"time":7090,"status":"running","event_ticks":2,"rows":[{"pid":9,"identity":"b","name":"new.exe","ms":70}]}),
            json!({"time":7091,"status":"running","event_ticks":2,"rows":[{"pid":9,"identity":"b","name":"new.exe","ms":70}]}),
        ];
        for v in values {
            let p = file(&dir, 22, v["time"].as_u64().unwrap() / 60);
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .unwrap();
            writeln!(f, "{v}").unwrap();
        }
        let r = read_at(&dir, 22, 60, 7100).unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 2);
        assert_eq!(r["rows"][0]["ms"], 70.0);
        let r = read_at(&dir, 22, 1, 7100).unwrap();
        assert_eq!(r["rows"].as_array().unwrap().len(), 1);
        for entry in std::fs::read_dir(&dir).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
        assert!(parse_name("settings.json").is_none());
        assert!(parse_name("cpu-22-../../x.jsonl").is_none());
    }
}
