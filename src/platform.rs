use crate::governor::Priority;
use serde_json::{json, Value};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::{
    Foundation::*,
    System::{Memory::*, Threading::*},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn err() -> String {
    std::io::Error::last_os_error().to_string()
}
fn open(pid: u32, rights: u32) -> Result<Handle, String> {
    let h = unsafe { OpenProcess(rights, 0, pid) };
    if h.is_null() {
        Err(err())
    } else {
        Ok(Handle(h))
    }
}
fn identity(h: HANDLE) -> Result<u64, String> {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (zeroed(), zeroed(), zeroed(), zeroed());
        if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
            return Err(err());
        }
        Ok((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64)
    }
}
pub fn groups_supported() -> bool {
    unsafe { GetActiveProcessorGroupCount() == 1 }
}
fn masks(h: HANDLE) -> Result<(usize, usize), String> {
    if !groups_supported() {
        return Err("Привязка для нескольких групп CPU пока недоступна".into());
    }
    let (mut process, mut system) = (0, 0);
    if unsafe { GetProcessAffinityMask(h, &mut process, &mut system) } == 0 {
        return Err(err());
    }
    Ok((process, system))
}
fn indices(mask: usize) -> Vec<usize> {
    (0..usize::BITS as usize)
        .filter(|i| mask & (1usize << i) != 0)
        .collect()
}
fn mask_for(cores: &[usize], allowed: usize) -> Result<usize, String> {
    if cores.is_empty() {
        return Err("Выберите хотя бы одно ядро".into());
    }
    let mut mask = 0usize;
    for &c in cores {
        if c >= usize::BITS as usize || allowed & (1usize << c) == 0 {
            return Err("Недопустимый номер CPU".into());
        }
        mask |= 1usize << c;
    }
    Ok(mask)
}
pub fn validate_cores(cores: &[usize]) -> Result<(), String> {
    mask_for(cores, masks(unsafe { GetCurrentProcess() })?.1).map(|_| ())
}
pub fn validate_excluded_cores(cores: &[usize]) -> Result<(), String> {
    let allowed = masks(unsafe { GetCurrentProcess() })?.1;
    let excluded = mask_for(cores, allowed)?;
    if allowed & !excluded == 0 {
        return Err("Cannot exclude every CPU".into());
    }
    Ok(())
}
pub fn compare_affinity(
    pid: u32,
    expected: &str,
    cores: &[usize],
    previous: Option<&[usize]>,
) -> Result<bool, String> {
    let h = open(pid, PROCESS_QUERY_INFORMATION | PROCESS_SET_INFORMATION)?;
    if identity(h.0)?.to_string() != expected {
        return Err("Процесс завершён или PID изменился".into());
    }
    let (current, allowed) = masks(h.0)?;
    if let Some(previous) = previous {
        if current != mask_for(previous, allowed)? {
            return Ok(false);
        }
    }
    let next = mask_for(cores, allowed)?;
    if unsafe { SetProcessAffinityMask(h.0, next) } == 0 {
        return Err(err());
    }
    Ok(true)
}
pub fn summary(pid: u32) -> Value {
    match open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(h) => {
            let m = masks(h.0).ok();
            let priority = priority_from_raw(unsafe { GetPriorityClass(h.0) });
            let mut critical = 1;
            let safe = unsafe { IsProcessCritical(h.0, &mut critical) } != 0 && critical == 0;
            let mut path = vec![0u16; 32768];
            let mut length = path.len() as u32;
            let path =
                if unsafe { QueryFullProcessImageNameW(h.0, 0, path.as_mut_ptr(), &mut length) }
                    != 0
                {
                    Some(String::from_utf16_lossy(&path[..length as usize]))
                } else {
                    None
                };
            let mut session = 0;
            let interactive =
                unsafe { ProcessIdToSessionId(pid, &mut session) } != 0 && session != 0;
            json!({"identity":identity(h.0).ok().map(|x|x.to_string()),"affinity":m.map(|m|indices(m.0)),"allowed":m.map(|m|indices(m.1)),"priority":priority,"path":path,"balance_safe":safe && interactive})
        }
        Err(_) => {
            json!({"identity":null,"affinity":null,"allowed":null,"priority":null,"path":null,"balance_safe":false})
        }
    }
}
fn priority_from_raw(value: u32) -> Option<Priority> {
    match value {
        IDLE_PRIORITY_CLASS => Some(Priority::Idle),
        BELOW_NORMAL_PRIORITY_CLASS => Some(Priority::BelowNormal),
        NORMAL_PRIORITY_CLASS => Some(Priority::Normal),
        ABOVE_NORMAL_PRIORITY_CLASS => Some(Priority::AboveNormal),
        HIGH_PRIORITY_CLASS => Some(Priority::High),
        REALTIME_PRIORITY_CLASS => Some(Priority::Realtime),
        _ => None,
    }
}
fn priority_raw(value: Priority) -> u32 {
    match value {
        Priority::Idle => IDLE_PRIORITY_CLASS,
        Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        Priority::Normal => NORMAL_PRIORITY_CLASS,
        Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        Priority::High => HIGH_PRIORITY_CLASS,
        Priority::Realtime => REALTIME_PRIORITY_CLASS,
    }
}
pub fn foreground_pid() -> u32 {
    let mut pid = 0;
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow(),
            &mut pid,
        );
    }
    pid
}
/// Identity and compare-before-write checks use the very same handle as the mutation.
/// A false result means another owner changed the priority; never overwrite it on restore.
pub fn priority(
    pid: u32,
    expected: &str,
    value: Priority,
    previous: Option<Priority>,
) -> Result<bool, String> {
    let h = open(
        pid,
        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION,
    )?;
    if identity(h.0)?.to_string() != expected {
        return Err("Процесс завершён или PID изменился".into());
    }
    let current = priority_from_raw(unsafe { GetPriorityClass(h.0) }).ok_or_else(err)?;
    if previous.is_some_and(|p| p != current) {
        return Ok(false);
    }
    if previous == Some(Priority::Normal) && value == Priority::BelowNormal {
        let mut critical = 1;
        let mut session = 0;
        if foreground_pid() == pid
            || unsafe { IsProcessCritical(h.0, &mut critical) } == 0
            || critical != 0
            || unsafe { ProcessIdToSessionId(pid, &mut session) } == 0
            || session == 0
        {
            return Ok(false);
        }
    }
    if unsafe { SetPriorityClass(h.0, priority_raw(value)) } == 0 {
        return Err(err());
    }
    Ok(true)
}
pub fn affinity(pid: u32, expected: &str, cores: &[usize]) -> Result<Value, String> {
    let h = open(pid, PROCESS_QUERY_INFORMATION | PROCESS_SET_INFORMATION)?;
    if identity(h.0)?.to_string() != expected {
        return Err("Процесс уже завершён или PID был переиспользован. Обновите выбор.".into());
    }
    let (_, allowed) = masks(h.0)?;
    let mask = mask_for(cores, allowed)?;
    if unsafe { SetProcessAffinityMask(h.0, mask) } == 0 {
        return Err(err());
    }
    let (actual, _) = masks(h.0)?;
    Ok(json!({"affinity":indices(actual)}))
}
/// Read-modify-write on the same process handle: do not overwrite other cores
/// with a stale mask submitted by the UI.
pub fn core_access(pid: u32, expected: &str, core: usize, enabled: bool) -> Result<Value, String> {
    let h = open(pid, PROCESS_QUERY_INFORMATION | PROCESS_SET_INFORMATION)?;
    if identity(h.0)?.to_string() != expected {
        return Err("Процесс завершён или PID изменился".into());
    }
    let (current, allowed) = masks(h.0)?;
    let bit = mask_for(&[core], allowed)?;
    let next = if enabled {
        current | bit
    } else {
        current & !bit
    };
    if next == 0 {
        return Err("Нельзя запретить последнее разрешённое ядро".into());
    }
    if next == current {
        return Ok(json!({"affinity":indices(current),"changed":false}));
    }
    if unsafe { SetProcessAffinityMask(h.0, next) } == 0 {
        return Err(err());
    }
    Ok(json!({"affinity":indices(masks(h.0)?.0),"changed":true}))
}

pub fn memory(pid: u32, expected: &str) -> Result<Value, String> {
    let h = open(pid, PROCESS_QUERY_INFORMATION)?;
    if identity(h.0)?.to_string() != expected {
        return Err("Процесс изменился. Выберите его заново.".into());
    }
    let mut regions = Vec::new();
    let mut address = 0usize;
    let mut visited = 0usize;
    loop {
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                h.0,
                address as _,
                &mut m,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            let code = unsafe { GetLastError() };
            if code != ERROR_INVALID_PARAMETER {
                return Err(format!(
                    "VirtualQueryEx: {}",
                    std::io::Error::from_raw_os_error(code as i32)
                ));
            }
            break;
        }
        if m.State == MEM_COMMIT {
            regions.push(json!({"base":format!("0x{:016X}",m.BaseAddress as usize),"size":m.RegionSize,"kind":match m.Type { MEM_IMAGE=>"image",MEM_MAPPED=>"mapped",_=>"private" },"protect":format!("0x{:X}",m.Protect)}));
        }
        visited += 1;
        let Some(next) = (m.BaseAddress as usize).checked_add(m.RegionSize) else {
            break;
        };
        if next <= address {
            break;
        }
        address = next;
        if visited >= 100000 {
            return Ok(json!({"regions":regions,"truncated":true}));
        }
    }
    Ok(json!({"regions":regions,"truncated":false}))
}
pub fn launch(path: &str, cores: &[usize]) -> Result<Value, String> {
    if !std::path::Path::new(path).is_absolute()
        || !path.to_ascii_lowercase().ends_with(".exe")
        || path.contains('\0')
    {
        return Err("Нужен полный путь к .exe".into());
    }
    let (_, allowed) = masks(unsafe { GetCurrentProcess() })?;
    let mask = mask_for(cores, allowed)?;
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = zeroed();
        if CreateProcessW(
            wide.as_ptr(),
            null_mut(),
            null(),
            null(),
            0,
            CREATE_SUSPENDED,
            null(),
            null(),
            &si,
            &mut pi,
        ) == 0
        {
            return Err(err());
        }
        let process = Handle(pi.hProcess);
        let thread = Handle(pi.hThread);
        if SetProcessAffinityMask(process.0, mask) == 0 {
            let e = err();
            TerminateProcess(process.0, 1);
            return Err(e);
        }
        if ResumeThread(thread.0) == u32::MAX {
            let e = err();
            TerminateProcess(process.0, 1);
            return Err(e);
        }
        Ok(json!({"pid":pi.dwProcessId,"affinity":indices(mask)}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_masks() {
        assert!(mask_for(&[], 15).is_err());
        assert!(mask_for(&[4], 15).is_err());
        assert!(mask_for(&[usize::BITS as usize], usize::MAX).is_err());
        assert_eq!(mask_for(&[0, 2, 2], 15).unwrap(), 5);
    }
    #[test]
    fn reads_own_memory_and_identity() {
        let id = std::process::id();
        let s = summary(id);
        assert!(s["identity"].is_string());
        let m = memory(id, s["identity"].as_str().unwrap()).unwrap();
        assert!(!m["regions"].as_array().unwrap().is_empty());
        assert!(memory(id, "wrong").is_err());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    #[test]
    fn affinity_round_trip_on_disposable_child() {
        if !groups_supported() {
            return;
        }
        let mut child = std::process::Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let tested = std::panic::catch_unwind(|| {
            let pid = child.id();
            let s = summary(pid);
            let identity = s["identity"].as_str().unwrap();
            let original: Vec<usize> = serde_json::from_value(s["affinity"].clone()).unwrap();
            let first = s["allowed"][0].as_u64().unwrap() as usize;
            assert!(affinity(pid, "stale", &[first]).is_err());
            assert!(affinity(pid, identity, &[]).is_err());
            let changed = affinity(pid, identity, &[first]).unwrap();
            assert_eq!(changed["affinity"], json!([first]));
            assert_eq!(summary(pid)["affinity"], json!([first]));
            assert!(core_access(pid, identity, first, false).is_err());
            assert!(core_access(pid, "stale", first, true).is_err());
            assert!(core_access(pid, identity, usize::BITS as usize, true).is_err());
            if let Some(second) = s["allowed"]
                .as_array()
                .unwrap()
                .get(1)
                .and_then(Value::as_u64)
            {
                core_access(pid, identity, second as usize, true).unwrap();
                core_access(pid, identity, first, false).unwrap();
                assert_eq!(summary(pid)["affinity"], json!([second]));
                // Repeated enable is idempotent, preserving the other CPU.
                core_access(pid, identity, second as usize, true).unwrap();
                assert_eq!(summary(pid)["affinity"], json!([second]));
            }
            affinity(pid, identity, &original).unwrap();
            assert!(priority(pid, "stale", Priority::BelowNormal, None).is_err());
            assert!(priority(pid, identity, Priority::BelowNormal, None).unwrap());
            assert_eq!(summary(pid)["priority"], json!(Priority::BelowNormal));
            assert!(!priority(pid, identity, Priority::High, Some(Priority::Normal)).unwrap());
            assert!(
                priority(pid, identity, Priority::Normal, Some(Priority::BelowNormal)).unwrap()
            );
            assert_eq!(summary(pid)["priority"], json!(Priority::Normal));
            assert!(!compare_affinity(pid, identity, &[first], Some(&[])).unwrap_or(false));
        });
        let _ = child.kill();
        let _ = child.wait();
        tested.unwrap();
    }
    #[test]
    fn launch_rejects_relative_paths_and_empty_affinity() {
        assert!(launch("cmd.exe", &[0]).is_err());
        assert!(launch("C:\\Windows\\System32\\cmd.exe", &[]).is_err());
    }
}
