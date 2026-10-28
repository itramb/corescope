#![windows_subsystem = "windows"]
mod activity;
mod activity_log;
mod desktop;
mod governor;
mod latency;
mod platform;
mod settings;
mod webview;
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    os::windows::process::CommandExt,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use sysinfo::{ProcessesToUpdate, System};
#[derive(Clone)]
struct App {
    snapshot: Arc<RwLock<Value>>,
    sensors: Arc<RwLock<Value>>,
    extended: Arc<RwLock<Value>>,
    token: Arc<String>,
    authority: Arc<String>,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
async fn guard(State(a): State<App>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let expected = format!("http://{}", a.authority);
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if host != Some(a.authority.as_str()) || origin.is_some_and(|o| o != expected) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if req.method() != axum::http::Method::GET
        && req
            .headers()
            .get("x-corescope-token")
            .and_then(|v| v.to_str().ok())
            != Some(a.token.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'".parse().unwrap());
    response
}
async fn index(State(a): State<App>) -> Html<String> {
    Html(
        include_str!("../assets/index.html")
            .replace("__TOKEN__", &a.token)
            .replace("/*__I18N__*/", include_str!("../assets/i18n.js"))
            .replace(
                "/*__AUTOMATION__*/",
                include_str!("../assets/automation.js"),
            )
            .replace(
                "/*__DIAGNOSTICS__*/",
                include_str!("../assets/diagnostics.js"),
            ),
    )
}
async fn snapshot(State(a): State<App>) -> Json<Value> {
    let mut s = a.snapshot.read().unwrap().clone();
    s["telemetry"] = a.sensors.read().unwrap().clone();
    let ext = a.extended.read().unwrap().clone();
    let mut list = s["telemetry"]["sensors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    list.extend(ext["sensors"].as_array().cloned().unwrap_or_default());
    s["telemetry"]["sensors"] = json!(list);
    s["telemetry"]["extended"] = ext;
    s["settings"] = json!(settings::get());
    s["automation"] = governor::snapshot();
    s["latency"] = latency::snapshot();
    s["activity_log"] = activity_log::status();
    Json(s)
}
type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn result(r: Result<Value, String>) -> ApiResult {
    r.map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({"error":e}))))
}
#[derive(Deserialize)]
struct Affinity {
    identity: String,
    cores: Vec<usize>,
}
async fn affinity(Path(pid): Path<u32>, Json(body): Json<Affinity>) -> ApiResult {
    result(platform::affinity(pid, &body.identity, &body.cores))
}
#[derive(Deserialize)]
struct PriorityRequest {
    identity: String,
    priority: governor::Priority,
}
async fn priority(Path(pid): Path<u32>, Json(body): Json<PriorityRequest>) -> ApiResult {
    result(governor::set_priority(pid, &body.identity, body.priority))
}
#[derive(Deserialize)]
struct AutomationRequest {
    config: governor::Config,
    revision: u64,
}
async fn automation(Json(body): Json<AutomationRequest>) -> ApiResult {
    result(governor::save(body.config, body.revision))
}
#[derive(Deserialize)]
struct LogRequest {
    core: usize,
    enabled: bool,
}
async fn log_core(Json(body): Json<LogRequest>) -> ApiResult {
    if body.core
        >= unsafe { windows_sys::Win32::System::Threading::GetActiveProcessorCount(0) } as usize
    {
        return result(Err("Invalid CPU".into()));
    }
    let mut s = settings::get();
    s.activity_log_cores.retain(|&c| c != body.core);
    if body.enabled {
        s.activity_log_cores.push(body.core);
    }
    result(settings::save(s).map(|s| json!(s)))
}
#[derive(Deserialize)]
struct HistoryQuery {
    core: usize,
    minutes: Option<u64>,
}
async fn activity_history(Query(q): Query<HistoryQuery>) -> ApiResult {
    result(
        tokio::task::spawn_blocking(move || activity_log::read(q.core, q.minutes.unwrap_or(60)))
            .await
            .unwrap_or_else(|e| Err(e.to_string())),
    )
}
#[derive(Deserialize)]
struct CoreAccess {
    identity: String,
    core: usize,
    enabled: bool,
}
async fn core_access(Path(pid): Path<u32>, Json(body): Json<CoreAccess>) -> ApiResult {
    result(platform::core_access(
        pid,
        &body.identity,
        body.core,
        body.enabled,
    ))
}
async fn application_core(
    State(a): State<App>,
    Path(pid): Path<u32>,
    Json(body): Json<CoreAccess>,
) -> ApiResult {
    let processes = a.snapshot.read().unwrap()["processes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    result(
        tokio::task::spawn_blocking(move || {
            governor::application_core(pid, &body.identity, body.core, body.enabled, &processes)
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string())),
    )
}
#[derive(Deserialize)]
struct MemoryQuery {
    identity: String,
}
async fn memory(Path(pid): Path<u32>, Query(q): Query<MemoryQuery>) -> ApiResult {
    result(
        tokio::task::spawn_blocking(move || platform::memory(pid, &q.identity))
            .await
            .unwrap_or_else(|e| Err(e.to_string())),
    )
}
#[derive(Deserialize)]
struct Launch {
    path: String,
    cores: Vec<usize>,
}
async fn launch(Json(b): Json<Launch>) -> ApiResult {
    result(platform::launch(&b.path, &b.cores))
}
async fn save_settings(Json(b): Json<settings::Settings>) -> ApiResult {
    result(settings::save(b).map(|s| json!(s)))
}
async fn start_sensors() -> ApiResult {
    result(settings::start_sensors().map(|_| json!({"ok":true})))
}
fn collect(a: App) {
    let mut system = System::new_all();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        system.refresh_cpu_all();
        system.refresh_memory();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let cpus: Vec<Value> = system
            .cpus()
            .iter()
            .enumerate()
            .map(|(id, c)| json!({"id":id,"usage":c.cpu_usage(),"mhz":c.frequency()}))
            .collect();
        let mut processes: Vec<Value> = system
            .processes()
            .iter()
            .map(|(pid, p)| {
                let mut v = platform::summary(pid.as_u32());
                v["pid"] = json!(pid.as_u32());
                v["name"] = json!(p.name().to_string_lossy());
                v["parent"] = json!(p.parent().map(|pid| pid.as_u32()));
                v["cpu"] = json!(p.cpu_usage() / cpus.len().max(1) as f32);
                v["memory"] = json!(p.memory());
                v["virtual_memory"] = json!(p.virtual_memory());
                v
            })
            .collect();
        governor::collect(&mut processes, system.global_cpu_usage() as f64);
        activity::remember(&processes);
        processes.sort_by(|a, b| {
            b["cpu"]
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&a["cpu"].as_f64().unwrap_or_default())
        });
        *a.snapshot.write().unwrap() = json!({"activity":activity::snapshot(),"ready":true,"time":now(),"host":System::host_name(),"cpu_name":system.cpus().first().map(|c|c.brand()),"cpu_usage":system.global_cpu_usage(),"cpus":cpus,"processes":processes,"memory_total":system.total_memory(),"memory_used":system.used_memory(),"affinity_supported":platform::groups_supported()});
        activity_log::record(
            &a.snapshot.read().unwrap(),
            &settings::get().activity_log_cores,
            &a.extended.read().unwrap(),
        );
    }
}
fn sensors(a: App, extended: bool) {
    let mut history: std::collections::HashMap<String, Vec<(u64, f64)>> =
        std::collections::HashMap::new();
    loop {
        if extended && !settings::get().extended_sensors {
            *a.extended.write().unwrap() = json!({"sensors":[],"provider":"disabled"});
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let mut command = std::process::Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                include_str!("../assets/sensors.ps1"),
            ])
            .env(
                "CORESCOPE_SENSOR_MODE",
                if extended { "extended" } else { "windows" },
            )
            .env(
                "CORESCOPE_HARDWARE_DIR",
                std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("hardware"),
            )
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let value = match command.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take().unwrap();
                let reader = std::thread::spawn(move || {
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    let _ = stdout.take(4 * 1024 * 1024).read_to_end(&mut bytes);
                    bytes
                });
                let start = std::time::Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Err(_) => {
                            let _ = child.kill();
                            break;
                        }
                        _ if start.elapsed() > Duration::from_secs(12) => {
                            let _ = child.kill();
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
                let _ = child.wait();
                reader
                    .join()
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            }
            Err(_) => None,
        };
        let mut completed = match value {
            Some(mut v) => {
                v["time"] = json!(now());
                if let Some(list) = v["sensors"].as_array_mut() {
                    for s in list {
                        if ["Temperature", "Clock", "Load", "Power", "Throttling"]
                            .contains(&s["kind"].as_str().unwrap_or(""))
                        {
                            if let (Some(id), Some(value)) =
                                (s["identifier"].as_str(), s["value"].as_f64())
                            {
                                let h = history.entry(id.to_owned()).or_default();
                                h.push((now(), value));
                                if h.len() > 120 {
                                    h.remove(0);
                                }
                                s["history"] = json!(h);
                            }
                        }
                    }
                }
                v
            }
            None => {
                json!({"provider":null,"sensors":[],"time":now(),"error":"Не удалось прочитать провайдер датчиков"})
            }
        };
        if let Some(list) = completed["sensors"].as_array_mut() {
            for s in list {
                s["sample_time"] = json!(now());
                s["source"] = json!(if extended {
                    "LibreHardwareMonitorLib"
                } else {
                    "Windows"
                });
            }
        }
        if extended {
            *a.extended.write().unwrap() = completed;
        } else {
            *a.sensors.write().unwrap() = completed;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
async fn run_server(
    ready: Option<std::sync::mpsc::Sender<String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .windows(2)
        .find(|p| p[0] == "--port")
        .map(|p| p[1].parse::<u16>())
        .transpose()?
        .unwrap_or(if ready.is_some() { 0 } else { 8899 });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let authority = listener.local_addr()?.to_string();
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    let a = App {
        snapshot: Arc::new(RwLock::new(json!({"ready":false}))),
        sensors: Arc::new(RwLock::new(json!({"sensors":[],"provider":null}))),
        extended: Arc::new(RwLock::new(json!({"sensors":[],"provider":null}))),
        token: Arc::new(random.iter().map(|b| format!("{b:02x}")).collect()),
        authority: Arc::new(authority.clone()),
    };
    let c = a.clone();
    std::thread::spawn(move || collect(c));
    let c = a.clone();
    std::thread::spawn(move || sensors(c, false));
    let c = a.clone();
    std::thread::spawn(move || sensors(c, true));
    let app = Router::new()
        .route("/", get(index))
        .route("/api/snapshot", get(snapshot))
        .route("/api/process/{pid}/memory", get(memory))
        .route("/api/process/{pid}/affinity", post(affinity))
        .route("/api/process/{pid}/priority", post(priority))
        .route("/api/automation", post(automation))
        .route("/api/activity/logging", post(log_core))
        .route("/api/activity/history", get(activity_history))
        .route("/api/process/{pid}/core", post(core_access))
        .route(
            "/api/process/{pid}/application/core",
            post(application_core),
        )
        .route("/api/launch", post(launch))
        .route("/api/settings", post(save_settings))
        .route("/api/sensors/start", post(start_sensors))
        .layer(middleware::from_fn_with_state(a.clone(), guard))
        .with_state(a);
    println!("CoreScope: http://{authority} — Ctrl+C to stop");
    if let Some(ready) = ready {
        let _ = ready.send(format!("http://{authority}"));
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            governor::stop();
        })
        .await?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--etw-watch") {
        if let (Some(pid), Some(name)) =
            (args.get(2).and_then(|s| s.parse::<u32>().ok()), args.get(3))
        {
            activity::watch_parent(pid, name);
        }
        return;
    }
    let headless = std::env::args().any(|a| a == "--no-open");
    if !headless && desktop::activate_existing() {
        return;
    }
    let _activity = activity::start();
    latency::start();
    if let Some(session) = &_activity {
        session.watch();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    if headless {
        if let Err(e) = runtime.block_on(run_server(None)) {
            eprintln!("{e}");
            std::process::exit(1)
        }
        return;
    }
    let (send, receive) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        if let Err(e) = run_server(Some(send)).await {
            desktop::error(&e.to_string());
        }
    });
    match receive.recv_timeout(Duration::from_secs(15)) {
        Ok(url) => {
            if let Err(e) = desktop::run(url) {
                desktop::error(&e)
            }
        }
        Err(e) => desktop::error(&format!("CoreScope could not start: {e}")),
    }
    runtime.shutdown_timeout(Duration::from_secs(2));
    governor::stop();
}
