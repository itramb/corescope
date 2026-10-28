//! Persistent process rules and reversible, opt-in workload management.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Mutex, OnceLock},
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    pub target: String,
    pub priority: Option<Priority>,
    pub cores: Option<Vec<usize>>,
    pub excluded_cores: Vec<usize>,
    pub include_descendants: bool,
    pub exclude_balance: bool,
    pub exclude_guard: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Balance {
    pub enabled: bool,
    pub system_threshold: f64,
    pub process_threshold: f64,
    pub recover_threshold: f64,
    pub sustain_seconds: u64,
    pub recover_seconds: u64,
}
impl Default for Balance {
    fn default() -> Self {
        Self {
            enabled: false,
            system_threshold: 85.0,
            process_threshold: 10.0,
            recover_threshold: 65.0,
            sustain_seconds: 3,
            recover_seconds: 5,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CpuGuard {
    pub enabled: bool,
    pub excluded_cores: Vec<usize>,
    pub process_threshold: f64,
    pub sustain_seconds: u64,
}
impl Default for CpuGuard {
    fn default() -> Self {
        Self {
            enabled: false,
            excluded_cores: vec![22],
            process_threshold: 3.0,
            sustain_seconds: 3,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub rules: Vec<Rule>,
    pub balance: Balance,
    pub cpu_guard: CpuGuard,
}
impl Config {
    pub fn validate(&mut self) -> Result<(), String> {
        let percent = |v: f64| v.is_finite() && v > 0.0 && v <= 100.0;
        let b = &self.balance;
        if !percent(b.system_threshold)
            || !percent(b.process_threshold)
            || !percent(b.recover_threshold)
            || b.recover_threshold >= b.system_threshold
            || !(1..=60).contains(&b.sustain_seconds)
            || !(1..=120).contains(&b.recover_seconds)
        {
            return Err("Invalid balance thresholds or durations".into());
        }
        let g = &mut self.cpu_guard;
        if !percent(g.process_threshold) || !(1..=60).contains(&g.sustain_seconds) {
            return Err("Invalid CPU guard threshold or duration".into());
        }
        g.excluded_cores.sort_unstable();
        g.excluded_cores.dedup();
        if g.excluded_cores.is_empty()
            || g.excluded_cores.iter().any(|&c| c >= usize::BITS as usize)
        {
            return Err("Invalid excluded CPUs".into());
        }
        if self.rules.len() > 256 {
            return Err("At most 256 rules are supported".into());
        }
        let mut targets = HashSet::new();
        for r in &mut self.rules {
            r.target = r.target.trim().replace('/', "\\");
            if !r.target.to_lowercase().ends_with(".exe")
                || r.target
                    .chars()
                    .any(|c| c.is_control() || "*?\"<>|".contains(c))
                || r.target.len() > 32768
                || (r.target.contains('\\') && !std::path::Path::new(&r.target).is_absolute())
                || !targets.insert(r.target.to_lowercase())
            {
                return Err("Use a unique executable name or absolute .exe path".into());
            }
            r.excluded_cores.sort_unstable();
            r.excluded_cores.dedup();
            if r.excluded_cores.iter().any(|&c| c >= usize::BITS as usize) {
                return Err("Invalid rule CPU exclusion".into());
            }
            if r.priority.is_none()
                && r.cores.is_none()
                && r.excluded_cores.is_empty()
                && !r.exclude_balance
                && !r.exclude_guard
            {
                return Err("A rule must have an action or exclusion".into());
            }
            if let Some(cores) = &mut r.cores {
                cores.sort_unstable();
                cores.dedup();
                if cores.is_empty() || cores.iter().any(|&c| c >= usize::BITS as usize) {
                    return Err("Invalid rule affinity".into());
                }
                if cores.iter().all(|c| r.excluded_cores.contains(c)) {
                    return Err("Rule affinity and CPU exclusions leave no allowed CPU".into());
                }
            }
        }
        Ok(())
    }
    fn rule_for(&self, p: &Sample) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|r| {
                p.path
                    .as_ref()
                    .is_some_and(|path| path.eq_ignore_ascii_case(&r.target))
            })
            .or_else(|| {
                self.rules
                    .iter()
                    .find(|r| r.target.eq_ignore_ascii_case(&p.name))
            })
    }
    fn rule_target(&self, target: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|r| r.target.eq_ignore_ascii_case(target))
    }
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    pid: u32,
    identity: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Sample {
    pub pid: u32,
    pub identity: Option<String>,
    pub name: String,
    pub path: Option<String>,
    pub cpu: f64,
    pub priority: Option<Priority>,
    pub affinity: Option<Vec<usize>>,
    pub balance_safe: bool,
    pub parent: Option<u32>,
}
impl Sample {
    fn key(&self) -> Option<Key> {
        self.identity.as_ref().map(|id| Key {
            pid: self.pid,
            identity: id.clone(),
        })
    }
}
trait Backend {
    fn priority(
        &mut self,
        key: &Key,
        value: Priority,
        previous: Option<Priority>,
    ) -> Result<bool, String>;
    fn affinity(
        &mut self,
        key: &Key,
        cores: &[usize],
        previous: Option<&[usize]>,
    ) -> Result<bool, String>;
}
struct Windows;
impl Backend for Windows {
    fn priority(&mut self, k: &Key, v: Priority, old: Option<Priority>) -> Result<bool, String> {
        crate::platform::priority(k.pid, &k.identity, v, old)
    }
    fn affinity(&mut self, k: &Key, v: &[usize], old: Option<&[usize]>) -> Result<bool, String> {
        crate::platform::compare_affinity(k.pid, &k.identity, v, old)
    }
}
#[derive(Default)]
struct Tracking {
    hot: Option<u64>,
    cool: Option<u64>,
    guard_hot: Option<u64>,
    retry: u64,
    restrained: bool,
    guarded: Option<(Vec<usize>, Vec<usize>)>,
}
#[derive(Default)]
struct Engine {
    tracked: HashMap<Key, Tracking>,
    events: VecDeque<Value>,
    stopped: bool,
    applications: HashMap<Key, Vec<String>>,
}
struct Tick {
    total: f64,
    foreground: u32,
    self_pid: u32,
    time: u64,
}
fn protected(p: &Sample) -> bool {
    !p.balance_safe
        || [
            "explorer.exe",
            "dwm.exe",
            "audiodg.exe",
            "csrss.exe",
            "winlogon.exe",
            "services.exe",
            "lsass.exe",
            "smss.exe",
            "system",
            "processlasso.exe",
            "processgovernor.exe",
        ]
        .contains(&p.name.to_ascii_lowercase().as_str())
}
fn family(samples: &[Sample], root: u32) -> HashSet<u32> {
    let mut ids = HashSet::from([root]);
    loop {
        let before = ids.len();
        for p in samples {
            if p.parent.is_some_and(|id| ids.contains(&id)) {
                ids.insert(p.pid);
            }
        }
        if ids.len() == before {
            return ids;
        }
    }
}
impl Engine {
    fn remember_applications(&mut self, cfg: &Config, samples: &[Sample]) {
        let alive: HashMap<_, _> = samples
            .iter()
            .map(|p| (p.pid, p.identity.as_deref()))
            .collect();
        self.applications.retain(|key, targets| {
            !targets.is_empty()
                && alive
                    .get(&key.pid)
                    .is_some_and(|id| id.is_none_or(|id| id == key.identity))
        });
        let by_pid: HashMap<_, _> = samples.iter().map(|p| (p.pid, p)).collect();
        for p in samples {
            let Some(key) = p.key() else { continue };
            for rule in cfg.rules.iter().filter(|r| r.include_descendants) {
                if p.path
                    .as_ref()
                    .is_some_and(|path| path.eq_ignore_ascii_case(&rule.target))
                    || p.name.eq_ignore_ascii_case(&rule.target)
                {
                    let targets = self.applications.entry(key.clone()).or_default();
                    if !targets.contains(&rule.target) {
                        targets.insert(0, rule.target.clone());
                    }
                }
            }
        }
        loop {
            let mut changed = false;
            for child in samples {
                let Some(key) = child.key() else { continue };
                let Some(parent) = child.parent.and_then(|pid| by_pid.get(&pid)) else {
                    continue;
                };
                let Some(parent_key) = parent.key() else {
                    continue;
                };
                // A surviving child cannot belong to a later owner of its parent's PID.
                if let (Ok(parent_birth), Ok(child_birth)) = (
                    parent_key.identity.parse::<u64>(),
                    key.identity.parse::<u64>(),
                ) {
                    if parent_birth > child_birth {
                        continue;
                    }
                }
                let parent_targets = self
                    .applications
                    .get(&parent_key)
                    .cloned()
                    .unwrap_or_default();
                if parent_targets.is_empty() {
                    continue;
                }
                let targets = self.applications.entry(key).or_default();
                for target in parent_targets {
                    if !targets.contains(&target) {
                        targets.push(target);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    fn event(&mut self, p: &Sample, action: &str, detail: Option<String>) {
        self.events.push_front(
            json!({"time":crate::now(),"pid":p.pid,"name":p.name,"action":action,"detail":detail}),
        );
        self.events.truncate(100);
    }
    fn tick(
        &mut self,
        cfg: &Config,
        samples: &mut [Sample],
        tick: Tick,
        backend: &mut impl Backend,
    ) {
        if self.stopped {
            return;
        }
        let Tick {
            total,
            foreground,
            self_pid,
            time,
        } = tick;
        let foreground_family = family(samples, foreground);
        let own_family = family(samples, self_pid);
        self.remember_applications(cfg, samples);
        let alive: HashMap<_, _> = samples
            .iter()
            .map(|p| (p.pid, p.identity.as_deref()))
            .collect();
        self.tracked.retain(|k, _| {
            alive
                .get(&k.pid)
                .is_some_and(|id| id.is_none_or(|id| id == k.identity))
        });
        for p in samples {
            let Some(key) = p.key() else { continue };
            let application_targets = self.applications.get(&key).cloned().unwrap_or_default();
            let inherited: Vec<_> = application_targets
                .iter()
                .filter_map(|target| cfg.rule_target(target))
                .filter(|r| r.include_descendants)
                .collect();
            let mut effective = cfg
                .rule_for(p)
                .cloned()
                .or_else(|| inherited.first().map(|r| (*r).clone()));
            if let Some(effective) = &mut effective {
                for parent in &inherited {
                    if effective.priority.is_none() {
                        effective.priority = parent.priority;
                    }
                    if effective.cores.is_none() {
                        effective.cores = parent.cores.clone();
                    }
                    effective.exclude_balance |= parent.exclude_balance;
                    effective.exclude_guard |= parent.exclude_guard;
                }
            }
            let rule = effective.as_ref();
            let mut excluded = rule.map(|r| r.excluded_cores.clone()).unwrap_or_default();
            for target in &application_targets {
                if let Some(r) = cfg.rule_target(target).filter(|r| r.include_descendants) {
                    excluded.extend(&r.excluded_cores);
                }
            }
            excluded.sort_unstable();
            excluded.dedup();
            let mut t = self.tracked.remove(&key).unwrap_or_default();
            let safe = !protected(p) && !own_family.contains(&p.pid);
            let balance_allowed = cfg.balance.enabled
                && safe
                && foreground != 0
                && !foreground_family.contains(&p.pid)
                && !rule.is_some_and(|r| r.exclude_balance || r.priority.is_some());
            let guard_allowed = cfg.cpu_guard.enabled
                && safe
                && excluded.is_empty()
                && !rule.is_some_and(|r| r.exclude_guard || r.cores.is_some());
            if t.restrained && p.priority.is_some() && p.priority != Some(Priority::BelowNormal) {
                t.restrained = false;
                t.retry = time + 10;
            }
            if t.guarded.as_ref().is_some_and(|(_, applied)| {
                p.affinity.is_some() && p.affinity.as_ref() != Some(applied)
            }) {
                t.guarded = None;
                t.retry = time + 10;
            }
            let low = total < cfg.balance.recover_threshold
                || p.cpu < cfg.balance.process_threshold / 2.0;
            if low {
                t.cool.get_or_insert(time);
            } else {
                t.cool = None;
            }
            let restore = t.restrained
                && (!balance_allowed
                    || t.cool.is_some_and(|start| {
                        time.saturating_sub(start) >= cfg.balance.recover_seconds
                    }));
            let restore_affinity = t.guarded.as_ref().is_some_and(|(original, applied)| {
                !guard_allowed
                    || original
                        .iter()
                        .filter(|c| !cfg.cpu_guard.excluded_cores.contains(c))
                        .copied()
                        .collect::<Vec<_>>()
                        != *applied
            });
            let enforce_exclusions = time >= t.retry;
            // Restore promptly on foreground/exclusion/disable, regardless of mutation backoff.
            if restore {
                match backend.priority(&key, Priority::Normal, Some(Priority::BelowNormal)) {
                    Ok(changed) => {
                        t.restrained = false;
                        t.hot = None;
                        t.retry = time + 10;
                        if changed {
                            p.priority = Some(Priority::Normal);
                            self.event(p, "restored", None);
                        }
                    }
                    Err(e) => {
                        if time >= t.retry {
                            self.event(p, "error", Some(e));
                        }
                        t.retry = time + 10;
                    }
                }
            }
            if restore_affinity {
                let (original, applied) = t.guarded.clone().unwrap();
                match backend.affinity(&key, &original, Some(&applied)) {
                    Ok(changed) => {
                        t.guarded = None;
                        t.guard_hot = None;
                        t.retry = time + 10;
                        if changed {
                            p.affinity = Some(original);
                            self.event(p, "affinity_restored", None);
                        }
                    }
                    Err(e) => {
                        if time >= t.retry {
                            self.event(p, "error", Some(e));
                        }
                        t.retry = time + 10;
                    }
                }
            }
            if time >= t.retry {
                if let Some(r) = rule {
                    if let Some(priority) = r.priority {
                        if p.priority != Some(priority) {
                            match backend.priority(&key, priority, None) {
                                Ok(true) => {
                                    p.priority = Some(priority);
                                    self.event(p, "rule_priority", None);
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    t.retry = time + 10;
                                    self.event(p, "error", Some(e));
                                }
                            }
                        }
                    }
                    if let Some(cores) = &r.cores {
                        let effective: Vec<_> = cores
                            .iter()
                            .filter(|c| !excluded.contains(c))
                            .copied()
                            .collect();
                        if effective.is_empty() {
                            t.retry = time + 10;
                            self.event(p,"error",Some("Rule affinity and application CPU exclusions leave no allowed CPU".into()));
                        } else if p.affinity.as_ref() != Some(&effective) && t.guarded.is_none() {
                            match backend.affinity(&key, &effective, None) {
                                Ok(true) => {
                                    p.affinity = Some(effective);
                                    self.event(p, "rule_affinity", None);
                                }
                                Ok(false) => {}
                                Err(e) => {
                                    t.retry = time + 10;
                                    self.event(p, "error", Some(e));
                                }
                            }
                        }
                    }
                }
                let hot = balance_allowed
                    && total >= cfg.balance.system_threshold
                    && p.cpu >= cfg.balance.process_threshold
                    && p.priority == Some(Priority::Normal);
                if hot && !t.restrained {
                    let start = *t.hot.get_or_insert(time);
                    if time.saturating_sub(start) >= cfg.balance.sustain_seconds {
                        match backend.priority(&key, Priority::BelowNormal, Some(Priority::Normal))
                        {
                            Ok(true) => {
                                t.restrained = true;
                                p.priority = Some(Priority::BelowNormal);
                                self.event(p, "restrained", None);
                            }
                            Ok(false) => {}
                            Err(e) => {
                                t.retry = time + 10;
                                self.event(p, "error", Some(e));
                            }
                        }
                        t.hot = None;
                    }
                } else {
                    t.hot = None;
                }
                if guard_allowed && p.cpu >= cfg.cpu_guard.process_threshold && t.guarded.is_none()
                {
                    let start = *t.guard_hot.get_or_insert(time);
                    if time.saturating_sub(start) >= cfg.cpu_guard.sustain_seconds {
                        if let Some(original) = p.affinity.clone() {
                            let next: Vec<_> = original
                                .iter()
                                .filter(|c| !cfg.cpu_guard.excluded_cores.contains(c))
                                .copied()
                                .collect();
                            if !next.is_empty() && next != original {
                                match backend.affinity(&key, &next, Some(&original)) {
                                    Ok(true) => {
                                        t.guarded = Some((original, next.clone()));
                                        p.affinity = Some(next);
                                        self.event(p, "cpu_guard", None);
                                    }
                                    Ok(false) => {}
                                    Err(e) => {
                                        t.retry = time + 10;
                                        self.event(p, "error", Some(e));
                                    }
                                }
                            }
                        }
                        t.guard_hot = None;
                    }
                } else {
                    t.guard_hot = None;
                }
            }
            if enforce_exclusions && !excluded.is_empty() {
                if let Some(current) = p.affinity.clone() {
                    let next: Vec<_> = current
                        .iter()
                        .filter(|c| !excluded.contains(c))
                        .copied()
                        .collect();
                    if next.is_empty() {
                        t.retry = time + 10;
                        self.event(
                            p,
                            "error",
                            Some(
                                "Application CPU exclusion would remove the last allowed CPU"
                                    .into(),
                            ),
                        );
                    } else if next != current {
                        match backend.affinity(&key, &next, Some(&current)) {
                            Ok(true) => {
                                p.affinity = Some(next);
                                self.event(p, "rule_affinity", None);
                            }
                            Ok(false) => {}
                            Err(error) => {
                                t.retry = time + 10;
                                self.event(p, "error", Some(error));
                            }
                        }
                    }
                }
            }
            self.tracked.insert(key, t);
        }
    }
    fn stop(&mut self, backend: &mut impl Backend) {
        self.stopped = true;
        for (k, t) in &self.tracked {
            if t.restrained {
                let _ = backend.priority(k, Priority::Normal, Some(Priority::BelowNormal));
            }
            if let Some((original, applied)) = &t.guarded {
                let _ = backend.affinity(k, original, Some(applied));
            }
        }
    }
}
struct State {
    config: Config,
    revision: u64,
    engine: Engine,
    start: Instant,
    error: Option<String>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        let path = crate::settings::directory().join("automation.json");
        let loaded = (|| -> Result<Config, String> {
            if !path.exists() {
                return Ok(Config::default());
            }
            let mut c: Config =
                serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            c.validate()?;
            Ok(c)
        })();
        let error = loaded.as_ref().err().cloned();
        Mutex::new(State {
            config: loaded.unwrap_or_default(),
            revision: 0,
            engine: Engine::default(),
            start: Instant::now(),
            error,
        })
    })
}
pub fn snapshot() -> Value {
    let s = state().lock().unwrap();
    json!({"config":s.config,"revision":s.revision,"error":s.error,"events":s.engine.events,
        "restrained":s.engine.tracked.values().filter(|t|t.restrained).count(),"guarded":s.engine.tracked.values().filter(|t|t.guarded.is_some()).count()})
}
pub fn save(mut config: Config, revision: u64) -> Result<Value, String> {
    validate_config(&mut config)?;
    let mut s = state().lock().unwrap();
    if s.revision != revision {
        return Err("Automation settings changed; reopen the editor".into());
    }
    save_locked(&mut s, config)
}
fn validate_config(config: &mut Config) -> Result<(), String> {
    config.validate()?;
    for r in &config.rules {
        if let Some(cores) = &r.cores {
            crate::platform::validate_cores(cores)?;
        }
        if !r.excluded_cores.is_empty() {
            crate::platform::validate_excluded_cores(&r.excluded_cores)?;
        }
    }
    if config.cpu_guard.enabled {
        crate::platform::validate_excluded_cores(&config.cpu_guard.excluded_cores)?;
    }
    Ok(())
}
fn save_locked(s: &mut State, config: Config) -> Result<Value, String> {
    let path = crate::settings::directory().join("automation.json");
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&config).unwrap())
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())?;
    s.config = config;
    s.revision += 1;
    s.error = None;
    for tracking in s.engine.tracked.values_mut() {
        tracking.retry = 0;
        tracking.hot = None;
        tracking.guard_hot = None;
        tracking.cool = None;
    }
    Ok(json!({"config":s.config,"revision":s.revision}))
}
fn update_application_rule(config: &mut Config, target: &str, core: usize, enabled: bool) {
    let index = config
        .rules
        .iter()
        .position(|r| r.target.eq_ignore_ascii_case(target));
    if let Some(index) = index {
        let r = &mut config.rules[index];
        r.include_descendants = true;
        if enabled {
            r.excluded_cores.retain(|&c| c != core);
            if let Some(cores) = &mut r.cores {
                if !cores.contains(&core) {
                    cores.push(core);
                }
            }
        } else if !r.excluded_cores.contains(&core) {
            r.excluded_cores.push(core);
        }
        if r.priority.is_none()
            && r.cores.is_none()
            && r.excluded_cores.is_empty()
            && !r.exclude_balance
            && !r.exclude_guard
        {
            config.rules.remove(index);
        }
    } else {
        let name = std::path::Path::new(target)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let mut rule = config
            .rules
            .iter()
            .find(|r| r.target.eq_ignore_ascii_case(&name))
            .cloned()
            .unwrap_or_default();
        rule.target = target.into();
        rule.include_descendants = true;
        if !enabled {
            rule.excluded_cores.push(core);
        } else {
            rule.excluded_cores.retain(|&c| c != core);
            if let Some(cores) = &mut rule.cores {
                if !cores.contains(&core) {
                    cores.push(core);
                }
            }
        }
        if rule.priority.is_some()
            || rule.cores.is_some()
            || !rule.excluded_cores.is_empty()
            || rule.exclude_balance
            || rule.exclude_guard
        {
            config.rules.push(rule);
        }
    }
}
fn application_members<'a>(
    engine: &Engine,
    target: &str,
    samples: &'a [Sample],
) -> Vec<&'a Sample> {
    let mut ids: HashSet<_> = samples
        .iter()
        .filter(|p| {
            p.path
                .as_ref()
                .is_some_and(|path| path.eq_ignore_ascii_case(target))
                || p.key()
                    .and_then(|key| engine.applications.get(&key))
                    .is_some_and(|targets| targets.iter().any(|t| t.eq_ignore_ascii_case(target)))
        })
        .map(|p| p.pid)
        .collect();
    let by_pid: HashMap<_, _> = samples.iter().map(|p| (p.pid, p)).collect();
    loop {
        let before = ids.len();
        for p in samples {
            let Some(parent) = p
                .parent
                .filter(|id| ids.contains(id))
                .and_then(|id| by_pid.get(&id))
            else {
                continue;
            };
            if let (Some(parent_birth), Some(child_birth)) = (
                parent.identity.as_ref().and_then(|i| i.parse::<u64>().ok()),
                p.identity.as_ref().and_then(|i| i.parse::<u64>().ok()),
            ) {
                if parent_birth > child_birth {
                    continue;
                }
            }
            ids.insert(p.pid);
        }
        if before == ids.len() {
            break;
        }
    }
    samples.iter().filter(|p| ids.contains(&p.pid)).collect()
}
fn apply_application_core(
    engine: &Engine,
    config: &Config,
    target: &str,
    members: &[&Sample],
    core: usize,
    enabled: bool,
    mut change: impl FnMut(&Sample) -> Result<bool, String>,
) -> Value {
    let mut changed = 0;
    let mut failed = Vec::new();
    for p in members {
        let conflict = enabled
            && (config.rule_for(p).is_some_and(|r| {
                !r.target.eq_ignore_ascii_case(target)
                    && r.cores.as_ref().is_some_and(|cores| !cores.contains(&core))
            }) || p
                .key()
                .and_then(|key| engine.applications.get(&key))
                .is_some_and(|targets| {
                    targets
                        .iter()
                        .filter_map(|t| config.rule_target(t))
                        .any(|r| r.include_descendants && r.excluded_cores.contains(&core))
                })
                || config
                    .rule_for(p)
                    .is_some_and(|r| r.excluded_cores.contains(&core)));
        let result = if p.identity.is_none() {
            Err("Process identity is unavailable".into())
        } else if conflict {
            Err("A separate process or parent application rule excludes this CPU".into())
        } else {
            change(p)
        };
        match result {
            Ok(true) => changed += 1,
            Ok(false) => {}
            Err(error) => failed.push(json!({"pid":p.pid,"name":p.name,"error":error})),
        }
    }
    json!({"matched":members.len(),"changed":changed,"failed":failed,"rule_target":target,"enabled":enabled,"core":core,"guard_may_reapply":enabled && config.cpu_guard.enabled && config.cpu_guard.excluded_cores.contains(&core)})
}
pub fn application_core(
    pid: u32,
    expected: &str,
    core: usize,
    enabled: bool,
    processes: &[Value],
) -> Result<Value, String> {
    crate::platform::validate_cores(&[core])?;
    let mut live = crate::platform::summary(pid);
    if live["identity"].as_str() != Some(expected) {
        return Err("Процесс завершён или PID изменился".into());
    }
    let path = live["path"]
        .as_str()
        .ok_or("Executable path is unavailable")?
        .to_owned();
    let mut samples: Vec<Sample> = processes
        .iter()
        .filter_map(|p| serde_json::from_value(p.clone()).ok())
        .collect();
    let old_parent = samples
        .iter()
        .find(|p| p.pid == pid && p.identity.as_deref() == Some(expected))
        .and_then(|p| p.parent);
    live["pid"] = json!(pid);
    live["name"] = json!(std::path::Path::new(&path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy());
    live["cpu"] = json!(0);
    live["parent"] = json!(old_parent);
    let anchor: Sample = serde_json::from_value(live).map_err(|e| e.to_string())?;
    let key = anchor.key().unwrap();
    if let Some(p) = samples.iter_mut().find(|p| p.pid == pid) {
        *p = anchor;
    } else {
        samples.push(anchor);
    }
    let mut s = state().lock().unwrap();
    let mut config = s.config.clone();
    s.engine.remember_applications(&config, &samples);
    let target = s
        .engine
        .applications
        .get(&key)
        .and_then(|targets| targets.first())
        .cloned()
        .unwrap_or(path);
    // Seed grouping before an allow operation potentially removes its final rule.
    let mut grouping = config.clone();
    if let Some(r) = grouping
        .rules
        .iter_mut()
        .find(|r| r.target.eq_ignore_ascii_case(&target))
    {
        r.include_descendants = true;
    } else {
        grouping.rules.push(Rule {
            target: target.clone(),
            include_descendants: true,
            ..Rule::default()
        });
    }
    s.engine.remember_applications(&grouping, &samples);
    let members = application_members(&s.engine, &target, &samples);
    update_application_rule(&mut config, &target, core, enabled);
    validate_config(&mut config)?;
    save_locked(&mut s, config.clone())?;
    let mut restoration_errors = HashMap::new();
    for p in &members {
        let Some(key) = p.key() else { continue };
        let Some(tracking) = s.engine.tracked.get_mut(&key) else {
            continue;
        };
        if let Some((original, applied)) = &tracking.guarded {
            match crate::platform::compare_affinity(p.pid, &key.identity, original, Some(applied)) {
                Ok(_) => {
                    tracking.guarded = None;
                    tracking.guard_hot = None;
                    tracking.retry = 0;
                }
                Err(error) => {
                    restoration_errors.insert(p.pid, error);
                }
            }
        }
    }
    let response = apply_application_core(
        &s.engine,
        &config,
        &target,
        &members,
        core,
        enabled,
        |p| {
            if let Some(error) = restoration_errors.get(&p.pid) {
                return Err(format!("Cannot restore automatic affinity before changing the application rule: {error}"));
            }
            crate::platform::core_access(p.pid, p.identity.as_deref().unwrap(), core, enabled)
                .map(|v| v["changed"].as_bool().unwrap_or(true))
        },
    );
    Ok(response)
}
pub fn collect(processes: &mut [Value], total: f64) {
    let mut samples: Vec<Sample> = processes
        .iter()
        .filter_map(|p| serde_json::from_value(p.clone()).ok())
        .collect();
    let mut s = state().lock().unwrap();
    let cfg = s.config.clone();
    let time = s.start.elapsed().as_secs();
    s.engine.tick(
        &cfg,
        &mut samples,
        Tick {
            total,
            foreground: crate::platform::foreground_pid(),
            self_pid: std::process::id(),
            time,
        },
        &mut Windows,
    );
    let by_pid: HashMap<_, _> = samples.iter().map(|p| (p.pid, p)).collect();
    for p in processes {
        if let Some(sample) = p["pid"].as_u64().and_then(|pid| by_pid.get(&(pid as u32))) {
            p["priority"] = json!(sample.priority);
            p["affinity"] = json!(sample.affinity);
            let t = sample.key().and_then(|k| s.engine.tracked.get(&k));
            p["restrained"] = json!(t.is_some_and(|t| t.restrained));
            p["guarded"] = json!(t.is_some_and(|t| t.guarded.is_some()));
            p["rule"] = json!(cfg.rule_for(sample).map(|r| &r.target));
            p["application_rule"] = json!(sample
                .key()
                .and_then(|key| s.engine.applications.get(&key))
                .and_then(|targets| targets.first()));
        }
    }
}
pub fn set_priority(pid: u32, identity: &str, value: Priority) -> Result<Value, String> {
    let mut s = state().lock().unwrap();
    crate::platform::priority(pid, identity, value, None)?;
    let time = s.start.elapsed().as_secs();
    if let Some(t) = s.engine.tracked.get_mut(&Key {
        pid,
        identity: identity.into(),
    }) {
        t.restrained = false;
        t.hot = None;
        t.retry = time + 10;
    }
    Ok(json!({"priority":value}))
}
pub fn stop() {
    state().lock().unwrap().engine.stop(&mut Windows);
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        priority: Priority,
        cores: Vec<usize>,
        changes: usize,
        fail: bool,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                priority: Priority::Normal,
                cores: vec![0, 1, 22],
                changes: 0,
                fail: false,
            }
        }
    }
    impl Backend for Fake {
        fn priority(
            &mut self,
            _: &Key,
            value: Priority,
            old: Option<Priority>,
        ) -> Result<bool, String> {
            if self.fail {
                return Err("denied".into());
            }
            if old.is_some_and(|p| p != self.priority) {
                return Ok(false);
            }
            self.priority = value;
            self.changes += 1;
            Ok(true)
        }
        fn affinity(
            &mut self,
            _: &Key,
            value: &[usize],
            old: Option<&[usize]>,
        ) -> Result<bool, String> {
            if self.fail {
                return Err("denied".into());
            }
            if old.is_some_and(|p| p != self.cores) {
                return Ok(false);
            }
            self.cores = value.to_vec();
            self.changes += 1;
            Ok(true)
        }
    }
    fn process() -> Sample {
        Sample {
            pid: 42,
            identity: Some("created-1".into()),
            name: "game.exe".into(),
            path: Some("C:\\Games\\game.exe".into()),
            cpu: 20.0,
            priority: Some(Priority::Normal),
            affinity: Some(vec![0, 1, 22]),
            balance_safe: true,
            parent: None,
        }
    }
    fn step(
        e: &mut Engine,
        c: &Config,
        p: &mut [Sample],
        foreground: u32,
        time: u64,
        b: &mut Fake,
    ) {
        e.tick(
            c,
            p,
            Tick {
                total: 95.0,
                foreground,
                self_pid: 999,
                time,
            },
            b,
        );
    }
    #[test]
    fn balance_waits_for_sustained_load_and_restores_foreground() {
        let mut c = Config::default();
        c.balance.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 2, &mut b);
        assert_eq!(b.changes, 0);
        step(&mut e, &c, &mut p, 8, 3, &mut b);
        assert_eq!(b.priority, Priority::BelowNormal);
        step(&mut e, &c, &mut p, 42, 4, &mut b);
        assert_eq!(b.priority, Priority::Normal);
    }
    #[test]
    fn restore_uses_hysteresis_and_does_not_overwrite_external_priority() {
        let mut c = Config::default();
        c.balance.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 3, &mut b);
        p[0].cpu = 0.0;
        step(&mut e, &c, &mut p, 8, 4, &mut b);
        step(&mut e, &c, &mut p, 8, 8, &mut b);
        assert_eq!(b.priority, Priority::BelowNormal);
        step(&mut e, &c, &mut p, 8, 9, &mut b);
        assert_eq!(b.priority, Priority::Normal);
        p[0].cpu = 20.0;
        step(&mut e, &c, &mut p, 8, 20, &mut b);
        step(&mut e, &c, &mut p, 8, 23, &mut b);
        b.priority = Priority::High;
        e.stop(&mut b);
        assert_eq!(b.priority, Priority::High);
    }
    #[test]
    fn guard_keeps_games_off_cpu22_after_load_drops_then_restores_on_disable() {
        let mut c = Config::default();
        c.cpu_guard.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        step(&mut e, &c, &mut p, 42, 0, &mut b);
        step(&mut e, &c, &mut p, 42, 3, &mut b);
        assert_eq!(b.cores, vec![0, 1]);
        p[0].cpu = 0.0;
        step(&mut e, &c, &mut p, 42, 30, &mut b);
        assert_eq!(b.cores, vec![0, 1]);
        c.cpu_guard.enabled = false;
        step(&mut e, &c, &mut p, 42, 31, &mut b);
        assert_eq!(b.cores, vec![0, 1, 22]);
    }
    #[test]
    fn explicit_rules_win_and_path_matching_is_case_insensitive() {
        let mut c = Config::default();
        c.balance.enabled = true;
        c.cpu_guard.enabled = true;
        c.rules = vec![
            Rule {
                target: "game.exe".into(),
                priority: Some(Priority::High),
                ..Rule::default()
            },
            Rule {
                target: "c:\\games\\GAME.EXE".into(),
                priority: Some(Priority::BelowNormal),
                cores: Some(vec![22]),
                ..Rule::default()
            },
        ];
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 30, &mut b);
        assert_eq!(b.cores, vec![22]);
        assert_eq!(b.priority, Priority::BelowNormal);
    }
    #[test]
    fn protected_foreground_children_and_excluded_processes_are_not_restrained() {
        let mut c = Config::default();
        c.balance.enabled = true;
        let mut e = Engine::default();
        let mut b = Fake::default();
        let mut p = vec![process()];
        p[0].parent = Some(8);
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 20, &mut b);
        assert_eq!(b.changes, 0);
        p[0].parent = None;
        p[0].balance_safe = false;
        step(&mut e, &c, &mut p, 8, 30, &mut b);
        step(&mut e, &c, &mut p, 8, 40, &mut b);
        assert_eq!(b.changes, 0);
        p[0].balance_safe = true;
        c.rules.push(Rule {
            target: "game.exe".into(),
            exclude_balance: true,
            exclude_guard: true,
            ..Rule::default()
        });
        step(&mut e, &c, &mut p, 8, 50, &mut b);
        step(&mut e, &c, &mut p, 8, 60, &mut b);
        assert_eq!(b.changes, 0);
    }
    #[test]
    fn reused_pid_drops_old_tracking_and_restarts_sustain_window() {
        let mut c = Config::default();
        c.cpu_guard.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 3, &mut b);
        p[0] = process();
        p[0].identity = Some("created-2".into());
        b.cores = vec![0, 1, 22];
        step(&mut e, &c, &mut p, 8, 10, &mut b);
        assert_eq!(b.cores, vec![0, 1, 22]);
        assert_eq!(e.tracked.len(), 1);
        c.cpu_guard.enabled = false;
        step(&mut e, &c, &mut p, 8, 11, &mut b);
        assert_eq!(b.changes, 1);
    }
    #[test]
    fn affinity_restore_respects_external_edits_and_never_removes_last_cpu() {
        let mut c = Config::default();
        c.cpu_guard.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake::default();
        p[0].affinity = Some(vec![22]);
        b.cores = vec![22];
        step(&mut e, &c, &mut p, 8, 0, &mut b);
        step(&mut e, &c, &mut p, 8, 3, &mut b);
        assert_eq!(b.changes, 0);
        p[0].affinity = Some(vec![0, 1, 22]);
        b.cores = vec![0, 1, 22];
        step(&mut e, &c, &mut p, 8, 4, &mut b);
        step(&mut e, &c, &mut p, 8, 7, &mut b);
        b.cores = vec![1];
        e.stop(&mut b);
        assert_eq!(b.cores, vec![1]);
    }
    #[test]
    fn errors_are_bounded_and_back_off() {
        let mut c = Config::default();
        c.rules.push(Rule {
            target: "game.exe".into(),
            priority: Some(Priority::High),
            ..Rule::default()
        });
        let mut e = Engine::default();
        let mut p = vec![process()];
        let mut b = Fake {
            fail: true,
            ..Fake::default()
        };
        for t in 0..1000 {
            step(&mut e, &c, &mut p, 8, t, &mut b);
        }
        assert_eq!(e.events.len(), 100);
    }
    #[test]
    fn validates_rules_and_round_trips_configuration() {
        let mut c = Config::default();
        c.rules.push(Rule {
            target: " GAME.EXE ".into(),
            cores: Some(vec![2, 0, 2]),
            ..Rule::default()
        });
        c.validate().unwrap();
        assert_eq!(c.rules[0].cores, Some(vec![0, 2]));
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&c).unwrap()).unwrap(),
            c
        );
        c.rules.push(c.rules[0].clone());
        assert!(c.validate().is_err());
        c.rules.pop();
        c.balance.recover_threshold = c.balance.system_threshold;
        assert!(c.validate().is_err());
        assert!(serde_json::from_str::<Priority>("\"invalid\"").is_err());
    }

    struct PerProcess {
        cores: HashMap<u32, Vec<usize>>,
        priorities: HashMap<u32, Priority>,
    }
    impl PerProcess {
        fn new(samples: &[Sample]) -> Self {
            Self {
                cores: samples
                    .iter()
                    .map(|p| (p.pid, p.affinity.clone().unwrap()))
                    .collect(),
                priorities: samples
                    .iter()
                    .map(|p| (p.pid, p.priority.unwrap()))
                    .collect(),
            }
        }
    }
    impl Backend for PerProcess {
        fn priority(
            &mut self,
            k: &Key,
            value: Priority,
            old: Option<Priority>,
        ) -> Result<bool, String> {
            if old.is_some_and(|p| self.priorities.get(&k.pid) != Some(&p)) {
                return Ok(false);
            }
            self.priorities.insert(k.pid, value);
            Ok(true)
        }
        fn affinity(
            &mut self,
            k: &Key,
            value: &[usize],
            old: Option<&[usize]>,
        ) -> Result<bool, String> {
            if old.is_some_and(|p| self.cores.get(&k.pid).map(Vec::as_slice) != Some(p)) {
                return Ok(false);
            }
            self.cores.insert(k.pid, value.to_vec());
            Ok(true)
        }
    }
    fn app_sample(pid: u32, born: u64, path: &str, parent: Option<u32>, cores: &[usize]) -> Sample {
        Sample {
            pid,
            identity: Some(born.to_string()),
            path: Some(path.into()),
            name: std::path::Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into(),
            parent,
            affinity: Some(cores.to_vec()),
            ..process()
        }
    }
    fn app_config() -> Config {
        Config {
            rules: vec![Rule {
                target: "C:\\Games\\game.exe".into(),
                excluded_cores: vec![22],
                include_descendants: true,
                ..Rule::default()
            }],
            ..Config::default()
        }
    }
    fn app_tick(e: &mut Engine, c: &Config, p: &mut [Sample], time: u64, b: &mut PerProcess) {
        e.tick(
            c,
            p,
            Tick {
                total: 95.0,
                foreground: 8,
                self_pid: 999,
                time,
            },
            b,
        );
    }
    #[test]
    fn application_rule_covers_all_instances_and_descendants_but_not_unrelated_helpers() {
        let c = app_config();
        let mut e = Engine::default();
        let mut p = vec![
            app_sample(1, 100, "C:\\Games\\game.exe", None, &[0, 1, 22]),
            app_sample(2, 101, "C:\\Games\\GAME.exe", None, &[1, 22]),
            app_sample(3, 200, "C:\\Common\\webview.exe", Some(1), &[0, 22]),
            app_sample(4, 300, "C:\\Common\\utility.exe", Some(3), &[1, 22]),
            app_sample(5, 200, "C:\\Common\\webview.exe", Some(90), &[0, 22]),
        ];
        let mut b = PerProcess::new(&p);
        app_tick(&mut e, &c, &mut p, 0, &mut b);
        assert_eq!(b.cores[&1], vec![0, 1]);
        assert_eq!(b.cores[&2], vec![1]);
        assert_eq!(b.cores[&3], vec![0]);
        assert_eq!(b.cores[&4], vec![1]);
        assert_eq!(b.cores[&5], vec![0, 22]);
        assert_eq!(application_members(&e, "C:\\Games\\game.exe", &p).len(), 4);
        p.retain(|s| s.pid > 2);
        p[0].affinity = Some(vec![0, 22]);
        b.cores.insert(3, vec![0, 22]);
        app_tick(&mut e, &c, &mut p, 1, &mut b);
        assert_eq!(b.cores[&3], vec![0]);
        p[0] = app_sample(3, 500, "C:\\Common\\webview.exe", Some(90), &[0, 22]);
        b.cores.insert(3, vec![0, 22]);
        app_tick(&mut e, &c, &mut p, 2, &mut b);
        assert_eq!(b.cores[&3], vec![0, 22]);
        assert!(!e.applications.contains_key(&p[0].key().unwrap()));
    }
    #[test]
    fn application_parent_pid_reuse_cannot_capture_older_unrelated_processes() {
        let c = app_config();
        let mut e = Engine::default();
        let mut p = vec![
            app_sample(1, 500, "C:\\Games\\game.exe", None, &[0, 22]),
            app_sample(2, 200, "C:\\Common\\helper.exe", Some(1), &[0, 22]),
        ];
        let mut b = PerProcess::new(&p);
        app_tick(&mut e, &c, &mut p, 0, &mut b);
        assert_eq!(b.cores[&2], vec![0, 22]);
        assert_eq!(application_members(&e, &c.rules[0].target, &p).len(), 1);
    }
    #[test]
    fn application_exclusions_compose_with_helper_priority_and_full_affinity() {
        let mut c = app_config();
        c.rules.push(Rule {
            target: "C:\\Common\\helper.exe".into(),
            priority: Some(Priority::High),
            cores: Some(vec![0, 1, 22]),
            ..Rule::default()
        });
        let mut e = Engine::default();
        let mut p = vec![
            app_sample(1, 100, "C:\\Games\\game.exe", None, &[0, 1, 22]),
            app_sample(2, 200, "C:\\Common\\helper.exe", Some(1), &[0, 1, 22]),
        ];
        let mut b = PerProcess::new(&p);
        app_tick(&mut e, &c, &mut p, 0, &mut b);
        assert_eq!(b.priorities[&2], Priority::High);
        assert_eq!(b.cores[&2], vec![0, 1]);
        c.rules[1].cores = Some(vec![22]);
        p[1].affinity = Some(vec![0, 22]);
        b.cores.insert(2, vec![0, 22]);
        app_tick(&mut e, &c, &mut p, 1, &mut b);
        assert_eq!(b.cores[&2], vec![0]);
        assert!(e.events.iter().any(|v| v["detail"]
            .as_str()
            .is_some_and(|s| s.contains("leave no allowed CPU"))));
    }
    #[test]
    fn explicit_application_exclusions_restore_automatic_guard_before_enforcement() {
        let mut c = Config::default();
        c.cpu_guard.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![app_sample(1, 100, "C:\\Games\\game.exe", None, &[0, 1, 22])];
        let mut b = PerProcess::new(&p);
        app_tick(&mut e, &c, &mut p, 0, &mut b);
        app_tick(&mut e, &c, &mut p, 3, &mut b);
        assert_eq!(b.cores[&1], vec![0, 1]);
        c.rules.push(Rule {
            target: "C:\\Games\\game.exe".into(),
            excluded_cores: vec![1],
            include_descendants: true,
            ..Rule::default()
        });
        app_tick(&mut e, &c, &mut p, 4, &mut b);
        assert_eq!(b.cores[&1], vec![0, 22]);
        assert!(e.tracked.values().all(|t| t.guarded.is_none()));
    }
    #[test]
    fn helper_rules_override_only_the_fields_they_actually_set() {
        let mut c = app_config();
        c.rules[0].priority = Some(Priority::AboveNormal);
        c.rules[0].cores = Some(vec![0, 1, 22]);
        c.rules.push(Rule {
            target: "C:\\Common\\priority.exe".into(),
            priority: Some(Priority::High),
            ..Rule::default()
        });
        c.rules.push(Rule {
            target: "C:\\Common\\affinity.exe".into(),
            cores: Some(vec![1, 22]),
            ..Rule::default()
        });
        let mut p = vec![
            app_sample(1, 100, "C:\\Games\\game.exe", None, &[0, 1, 22]),
            app_sample(2, 200, "C:\\Common\\priority.exe", Some(1), &[0, 1, 22]),
            app_sample(3, 300, "C:\\Common\\affinity.exe", Some(1), &[0, 1, 22]),
        ];
        let mut e = Engine::default();
        let mut b = PerProcess::new(&p);
        app_tick(&mut e, &c, &mut p, 0, &mut b);
        assert_eq!(b.priorities[&2], Priority::High);
        assert_eq!(b.cores[&2], vec![0, 1]);
        assert_eq!(b.priorities[&3], Priority::AboveNormal);
        assert_eq!(b.cores[&3], vec![1]);
    }
    #[test]
    fn application_rule_merge_preserves_name_rule_and_other_actions() {
        let mut c = Config::default();
        c.rules.push(Rule {
            target: "game.exe".into(),
            priority: Some(Priority::AboveNormal),
            cores: Some(vec![0, 22]),
            exclude_balance: true,
            ..Rule::default()
        });
        update_application_rule(&mut c, "C:\\Games\\game.exe", 22, false);
        c.validate().unwrap();
        let r = c.rule_target("C:\\Games\\game.exe").unwrap();
        assert_eq!(r.priority, Some(Priority::AboveNormal));
        assert!(r.include_descendants && r.exclude_balance);
        assert_eq!(r.cores, Some(vec![0, 22]));
        assert_eq!(r.excluded_cores, vec![22]);
        assert_eq!(c.rules[0].target, "game.exe");
        update_application_rule(&mut c, "C:\\Games\\game.exe", 22, true);
        c.validate().unwrap();
        assert!(c.rules[1].excluded_cores.is_empty());
        assert_eq!(c.rules[1].priority, Some(Priority::AboveNormal));
        c.rules[1].cores = Some(vec![22]);
        update_application_rule(&mut c, "C:\\Games\\game.exe", 22, false);
        assert!(c.validate().is_err());
    }
    #[test]
    fn application_result_reports_partial_failures_and_guard_reapplication() {
        let mut c = app_config();
        c.cpu_guard.enabled = true;
        let mut e = Engine::default();
        let mut p = vec![
            app_sample(1, 100, "C:\\Games\\game.exe", None, &[0, 22]),
            app_sample(2, 200, "C:\\Common\\helper.exe", Some(1), &[22]),
            app_sample(3, 300, "C:\\Common\\protected.exe", Some(1), &[0, 22]),
        ];
        p[2].identity = None;
        e.remember_applications(&c, &p);
        let members = application_members(&e, &c.rules[0].target, &p);
        let r = apply_application_core(&e, &c, &c.rules[0].target, &members, 22, false, |s| {
            if s.pid == 2 {
                Err("last CPU".into())
            } else {
                Ok(true)
            }
        });
        assert_eq!(r["matched"], 3);
        assert_eq!(r["changed"], 1);
        assert_eq!(r["failed"].as_array().unwrap().len(), 2);
        let r = apply_application_core(
            &e,
            &Config {
                rules: vec![],
                ..c.clone()
            },
            &c.rules[0].target,
            &members,
            22,
            true,
            |_| Ok(true),
        );
        assert_eq!(r["guard_may_reapply"], true);
    }
}
