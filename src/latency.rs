//! A timer wake-up probe. This measures scheduling/timer lateness, not DPC latency.
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::*, System::Threading::*};
#[derive(Default)]
struct State {
    history: VecDeque<Value>,
    peak: f64,
    error: Option<String>,
}
fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}
pub fn snapshot() -> Value {
    let s = state().lock().unwrap();
    json!({"history":s.history,"peak_us":s.peak,"error":s.error,"interval_ms":10,"kind":"timer_wakeup"})
}
pub fn latest() -> Option<Value> {
    state().lock().unwrap().history.back().cloned()
}
pub fn start() {
    std::thread::spawn(|| unsafe {
        let timer = CreateWaitableTimerExW(
            std::ptr::null(),
            std::ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        );
        if timer.is_null() {
            state().lock().unwrap().error = Some(std::io::Error::last_os_error().to_string());
            return;
        }
        let mut start = Instant::now();
        let mut sum = 0.0;
        let mut count = 0u64;
        let mut peak = 0f64;
        loop {
            let before = Instant::now();
            let due = -100_000i64;
            if SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 0) == 0
                || WaitForSingleObject(timer, 1000) != WAIT_OBJECT_0
            {
                state().lock().unwrap().error = Some(std::io::Error::last_os_error().to_string());
                break;
            }
            let late = before
                .elapsed()
                .saturating_sub(Duration::from_millis(10))
                .as_secs_f64()
                * 1_000_000.0;
            // Resume/suspend gaps are not useful estimates of interactive latency.
            if late < 1_000_000.0 {
                sum += late;
                count += 1;
                peak = peak.max(late);
            }
            if start.elapsed() >= Duration::from_secs(1) {
                let time = crate::now();
                let mut s = state().lock().unwrap();
                s.peak = s.peak.max(peak);
                if count > 0 {
                    s.history.push_back(json!({"time":time,"average_us":sum/count as f64,"max_us":peak,"samples":count}));
                }
                while s
                    .history
                    .front()
                    .is_some_and(|v| v["time"].as_u64().unwrap_or(0) < time.saturating_sub(3600))
                    || s.history.len() > 3600
                {
                    s.history.pop_front();
                }
                sum = 0.0;
                count = 0;
                peak = 0.0;
                start = Instant::now();
            }
        }
        CloseHandle(timer);
    });
}

/// Raw-timestamp ETW PerfInfo records store the entry QPC at offset zero.
pub fn interrupt_duration(
    opcode: u8,
    bytes: &[u8],
    end: i64,
    frequency: i64,
) -> Option<(bool, f64)> {
    if ![66, 67, 68, 69].contains(&opcode) || bytes.len() < 12 || frequency <= 0 {
        return None;
    }
    let start = i64::from_le_bytes(bytes[..8].try_into().ok()?);
    let ticks = end.checked_sub(start)?;
    if ticks < 0 || ticks > frequency * 60 {
        return None;
    }
    Some((opcode == 67, ticks as f64 * 1_000_000.0 / frequency as f64))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_raw_qpc_and_rejects_malformed_records() {
        let mut b = vec![0; 16];
        b[..8].copy_from_slice(&1000i64.to_le_bytes());
        assert_eq!(
            interrupt_duration(68, &b, 1250, 1_000_000),
            Some((false, 250.0))
        );
        assert_eq!(
            interrupt_duration(67, &b, 1020, 1_000_000),
            Some((true, 20.0))
        );
        assert!(interrupt_duration(67, &b, 900, 1_000_000).is_none());
        assert!(interrupt_duration(36, &b, 1250, 1_000_000).is_none());
        assert!(interrupt_duration(68, &b[..7], 1250, 1_000_000).is_none());
    }
}
