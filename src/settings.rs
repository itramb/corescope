use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: String,
    pub close_to_tray: bool,
    pub extended_sensors: bool,
    pub activity_log_cores: Vec<usize>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "ru".into(),
            close_to_tray: true,
            extended_sensors: true,
            activity_log_cores: Vec::new(),
        }
    }
}
pub fn directory() -> std::path::PathBuf {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(pair) = args.windows(2).find(|p| p[0] == "--data-dir") {
        let dir = std::path::PathBuf::from(&pair[1]);
        assert!(dir.is_absolute(), "--data-dir must be absolute");
        return dir;
    }
    use std::os::windows::ffi::OsStringExt;
    unsafe {
        let mut raw = std::ptr::null_mut();
        if windows_sys::Win32::UI::Shell::SHGetKnownFolderPath(
            &windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData,
            0,
            std::ptr::null_mut(),
            &mut raw,
        ) >= 0
        {
            let mut len = 0;
            while *raw.add(len) != 0 {
                len += 1;
            }
            let dir = std::path::PathBuf::from(std::ffi::OsString::from_wide(
                std::slice::from_raw_parts(raw, len),
            ));
            windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());
            return dir.join("CoreScope");
        }
    }
    std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LocalAppData unavailable"))
        .join("CoreScope")
}
fn path() -> std::path::PathBuf {
    directory().join("settings.json")
}
fn state() -> &'static Mutex<Settings> {
    static STATE: OnceLock<Mutex<Settings>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(
            std::fs::read(path())
                .ok()
                .and_then(|s| serde_json::from_slice(&s).ok())
                .unwrap_or_default(),
        )
    })
}
pub fn get() -> Settings {
    state().lock().unwrap().clone()
}
pub fn save(value: Settings) -> Result<Settings, String> {
    if !["ru", "en"].contains(&value.language.as_str()) {
        return Err("Invalid language".into());
    }
    if value.activity_log_cores.len() > 4
        || value
            .activity_log_cores
            .iter()
            .any(|&c| c >= usize::BITS as usize)
    {
        return Err("Select up to four CPUs for logging".into());
    }
    let mut guard = state().lock().unwrap();
    let p = path();
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = p.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&value).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &p).map_err(|e| e.to_string())?;
    *guard = value.clone();
    Ok(value)
}
pub fn start_sensors() -> Result<(), String> {
    let mut s = get();
    s.extended_sensors = true;
    save(s)?;
    Ok(())
}
