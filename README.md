# CoreScope

A Windows desktop application for seeing which processes execute on each CPU, controlling where they may run, and monitoring memory and hardware sensors. Built with Rust and an embedded WebView2 interface, with an isometric system map, a light theme, English/Russian localization, and a system tray menu.

[Download the installer](https://github.com/Sobolev5/corescope/releases/latest) · [All releases](https://github.com/Sobolev5/corescope/releases) · [Report an issue](https://github.com/Sobolev5/corescope/issues)

## Features

| Area | What you can do |
| --- | --- |
| **CPU activity** | Click a logical CPU to see processes scheduled there during a one-second ETW window, with CPU time in milliseconds, utilization, and short history graphs. |
| **Activity filters** | Show any activity, CPU ≥ 1%, or CPU ≥ 5%; search by process name or PID; see visible/total counts. |
| **Allowed / Denied** | Switch from actual activity to scheduling permissions. Allow or deny the selected CPU directly from a process row. |
| **Full affinity editor** | Select a process and edit its entire CPU affinity mask. The last allowed CPU cannot be removed. |
| **Launch on CPUs** | Start an executable with affinity applied before its main thread resumes. |
| **Process explorer** | Browse all processes independently of the selected CPU; search, sort by CPU or RAM, filter, and reset the view. |
| **Process filters** | Show CPU ≥ 1%, RAM ≥ 100 MiB, or processes with restricted affinity. |
| **Virtual memory** | Inspect committed regions, virtual addresses, sizes, image/private/mapped types, and protection flags. |
| **Hardware telemetry** | View available temperatures, fan speeds, power, voltage, clocks, and loads through Windows collectors and bundled LibreHardwareMonitorLib. |
| **Sensor history** | Inspect the latest 120 successful readings per sensor, with a graph, interval minimum/maximum, source, and sample time. |
| **Desktop integration** | Use the tray menu, close-to-tray behavior, optional startup at sign-in, and persistent EN/RU settings. |
| **Distribution** | Install with the English/Russian Windows installer or extract the portable ZIP. |

## Workload protection and diagnostics (development build)

The new **Rules and CPU protection** editor adds persistent CPU priority and affinity rules, automatic priority balancing, and automatic CPU exclusion for heavy applications. Both automatic modes start **disabled**. The **Exclude CPU 22 for heavy processes** switch under the system map enables or disables that exclusion directly. An orange border appears only while exclusion is enabled; it does not mean the CPU itself is disabled in Windows. If Process Lasso is running, the editor shows a conflict notice; pause its corresponding rules before enabling a second controller.

- Rules match an exact executable name or full path, ignoring case; a full-path rule takes precedence over a name rule. Rules are enforced each sampling cycle while CoreScope runs, including after an application restarts. Explicit priority/affinity rules take precedence over the corresponding automatic mode. Removing a rule stops enforcement and does not undo its previous changes.
- CPU protection defaults to excluding logical **CPU 22** after a process uses at least **3% of the whole CPU for 3 seconds**. It covers foreground games too and keeps the restriction until the process exits or protection is disabled. It removes CPUs from the process's existing mask; it never adds CPUs or removes the last one. GPU activity alone is not a trigger. For restrictions before an executable's first instruction, use **Launch on CPUs → All except CPU 22**; rules and automatic detection act after startup.
- Balancing waits for **85% system CPU** and **10% process CPU for 3 seconds**, then changes Normal to Below normal. Recovery requires system CPU below **65%** or process CPU below half its trigger for **5 seconds**. Foreground applications and descendants, services, critical processes, the shell and CoreScope descendants are excluded. Foreground selection, an exclusion or disabling balancing also triggers restoration.
- Temporary automatic settings are restored on normal shutdown or when disabled, but only if the process still has the settings CoreScope applied. External changes are respected. A crash or forced termination cannot guarantee restoration. Rules and automation settings are saved separately in `%LOCALAPPDATA%\CoreScope\automation.json`.

The process table now sorts by name and by the numeric list of allowed CPUs in either direction. **Executed on CPUs** comes from the one-second ETW execution window, independent of affinity. Select a CPU and switch to **Last 20 seconds** to see recent execution, including exited processes; names are retained when the process was observed in a snapshot. Very short-lived processes may be identified only by PID. Access to the kernel ETW session normally requires administrator privileges.

In **Allowed / Denied**, **Group by application** combines executable instances and identifies common shared helper processes by their parent application. **Deny for application / Allow for application** updates a persistent rule for the executable path and all its descendants, including later launches. Excluding a CPU removes only that CPU from each process's existing mask. Shared helpers from unrelated applications are not combined. A helper already associated with an application retains that association after its parent exits, while CoreScope remains running. Expand **Individual processes** for temporary per-process controls. The result reports any processes Windows refused to change; excluding the final allowed CPU is always rejected.

**Record last hour** records the selected CPU in minute files under `%LOCALAPPDATA%\CoreScope\activity-log`. Up to four CPUs can be recorded. The viewer shows per-process totals, individual timestamped samples and JSON export. Samples include CPU load, DPC/ISR maxima and timer wake-up measurements. These are one-second observations with possible gaps, not a complete scheduler trace. Repeated event windows are excluded from aggregate totals. Missing ETW, lost events and truncated rows remain visible. Old minute files are deleted while CoreScope runs (at most one extra partial minute is retained); each minute is capped at 1 MiB, and each observation at 256 processes. The viewer filters to the last hour even after an application restart. No history exists before recording was enabled.

**CPU diagnostics** is below the process panels; the logical CPU cards appear once under the system map. It separates logical CPU utilization from hardware sensor names. LibreHardwareMonitor CPU temperatures, clocks, loads and power expose their source, sample time and observed minima/maxima. Distance-to-TjMax is not treated as a core temperature. CPU Package temperature is not substituted with an ACPI or drive reading. A throttling status is shown only if the provider supplies an explicit binary throttling flag; unsupported values say **No data**, even at high temperature. Physical sensor core indices are not assumed to match Windows CPU numbers.

**System latency** measures overshoot of a 10 ms high-resolution waitable timer at normal scheduling priority, with one-second maxima/averages, a session peak and an in-memory one-hour history. It measures timer/scheduling lateness, not audio latency. ETW additionally reports per-CPU DPC and ISR counts and maximum routine elapsed time since the previous snapshot, plus session peaks. This is a diagnostic subset, not full LatencyMon parity: it does not attribute routines to drivers, measure audio dropouts or report hard page faults. DPC and ISR execution can overlap with the context-switch CPU time accounting; their durations must not be added to process execution time.

For an isolated development profile, use `corescope.exe --no-open --port 8899 --data-dir C:\\path\\to\\test-profile`. The data directory must be absolute. Neither Windows power plans nor interrupt affinity are changed.

## Screenshots

These screenshots show **CoreScope 0.6.1 in English**, with real process and sensor readings. They are captured directly from the application interface at 2× pixel density and saved as lossless PNGs. Click an image to view it at full resolution. Readings vary with workload and hardware.

### System map and per-CPU activity

Select a CPU on the isometric map to open **Activity**. The panel shows scheduled execution time, delivery delay, idle time, process history, and the activity threshold. **Allowed** and **Denied** remain one click away.

![CoreScope 0.6.1: isometric system map and CPU 3 activity with the CPU ≥ 1% filter](docs/screenshots/activity-en.png)

### Process filters

The global list stays independent of the selected CPU. Combine name/PID search with a CPU, RAM, or affinity filter, then open a process to inspect its working set and choose its allowed CPUs. **Refresh memory map** loads its committed virtual-memory regions.

![CoreScope 0.6.1: RAM filter, visible/total process count, CPU usage and working set](docs/screenshots/processes-en.png)

### Affinity editor and virtual memory

Inspect the selected process's allowed CPUs and committed memory regions. Affinity changes take effect only when you click **Apply**.

<a href="docs/screenshots/memory-en.png"><img src="docs/screenshots/memory-en.png" width="585" alt="CoreScope 0.6.1: CPU affinity editor and committed virtual-memory regions"></a>

### Temperatures and sensor history

Sensor cards include the source, sample time, history, and interval minimum/maximum. ACPI thermal zones are labeled separately from CPU temperatures. This machine exposes ACPI and SSD temperatures; unsupported CPU temperatures are reported as unavailable.

<a href="docs/screenshots/sensors-en.png"><img src="docs/screenshots/sensors-en.png" width="585" alt="CoreScope 0.6.1: ACPI and SSD temperatures with history graphs and source labels"></a>

## Install

Download `CoreScope-Setup-0.6.1.exe` from [Releases](https://github.com/Sobolev5/corescope/releases/latest). The English/Russian installer creates shortcuts and an uninstall entry. Startup at sign-in is optional and starts the application in the tray.

If Microsoft Edge WebView2 Runtime is missing, setup downloads it from Microsoft; an internet connection is needed for that step. The installer is currently unsigned.

**Requirements:** Windows 10/11 x64, Microsoft Edge WebView2 Runtime, and .NET Framework 4.7.2 or later for extended sensors.

For a portable installation, extract the entire ZIP and run `corescope.exe`. Keep `WebView2Loader.dll` and the `hardware` folder beside the executable.

## Use

1. Click a CPU tile on the map or a utilization card. **Activity** opens by default and shows processes that executed on that CPU over a one-second ETW window, ordered by CPU time. Rows include milliseconds, utilization, and a short history. Sorting stays stable while hovered.
2. Switch to **Allowed** or **Denied** to manage scheduling permissions. **Deny** removes only the selected CPU; **Allow** restores it. The overall process list stays independent of the selected CPU.
3. Click a process name to inspect its memory or edit its full CPU affinity mask. Select CPUs and click **Apply**.
4. Use **Launch on CPUs** to start an `.exe` on selected CPUs. Command-line arguments are not supported yet.

Closing the window keeps monitoring active in the tray by default. Click the tray icon or launch CoreScope again to restore the window. Choose **Exit** from the tray menu to quit. Close-to-tray can be disabled in **Settings**.

Switch between **EN** and **RU** in the toolbar or tray menu. Preferences are stored in `%LOCALAPPDATA%\CoreScope\settings.json`. Windows error messages and hardware-provided names retain their original language.

### Command-line options

Run these commands from the folder containing the executable:

```powershell
# Open the desktop window
.\corescope.exe

# Start in the system tray
.\corescope.exe --tray

# Run only the local API on a fixed port
.\corescope.exe --no-open --port 8899
```

The desktop window normally uses a free local port. Starting another desktop instance restores the existing window. The API-only mode does not create a window or tray icon; stop its process when finished.

## Hardware sensors

Two independent collectors run in the background:

| Source | Readings |
| --- | --- |
| Windows ACPI / Storage | Firmware thermal zones and supported drive temperatures |
| Integrated LibreHardwareMonitorLib | Available CPU/GPU/motherboard temperatures, fans, power, voltage, clocks, and load |

Each reading includes its source and sample time. Disabling **Extended sensors** leaves the Windows collector active. CPU/process data refreshes about once a second; sensors refresh every five seconds plus collection time.

Some CPU sensors require PawnIO and elevated access. CoreScope does not install drivers or elevate itself automatically. Unsupported zero CPU-temperature readings are excluded; the interface reports that CPU temperature is unavailable. Sensor errors can be inspected under **Sensor diagnostics**.

The extended collector reports whether PawnIO is installed, whether administrator access is available, the library version, and the number of temperature sensors/readings in `telemetry.extended.diagnostics`. Missing CPU temperatures now distinguish a missing driver from missing access or unsupported readings. A recognized CPU can expose named thermal sensors with null values until low-level hardware access is available. Obtain PawnIO separately from [its official website](https://pawnio.eu/), then restart CoreScope with the required privileges.

History is held in memory until CoreScope exits. Missing samples are not replaced with invented readings.

The sensor panel groups readings by device (CPU, GPU, drives, memory, motherboard and ACPI), with collapsible sections for temperatures, clocks, load, fans, power and voltage. Expansion state, scroll position and keyboard focus survive live updates. The panel is rebuilt only when readings or status change. Temperature margins, sensor resolution and thermal limits are shown separately as sensor parameters and are excluded from live temperature maxima.

## What the data means

- **Activity and affinity are separate.** Activity uses ETW context-switch events to account for scheduled thread time over one second, with the delivery delay shown in the panel. It is not an instantaneous list or a breakdown of interrupt/DPC time. Idle and unattributed time are shown separately. ETW normally requires an administrator launch; when unavailable, the permissions tabs still work. Allowed/Denied show where Windows may schedule a process, including sleeping processes. A multithreaded process may execute on several CPUs.
- **CPU numbers identify logical processors.** They do not necessarily correspond to separate physical cores. The system map is schematic; the RAM bars show usage, not detected DIMM slots.
- **Memory addresses are virtual.** The memory view uses `VirtualQueryEx`; it does not expose physical RAM addresses, DIMM placement, memory contents, or CPU registers. Committed memory and the resident working set are different measurements.
- **ACPI is a firmware thermal zone.** It is not presented as a per-core temperature. The top temperature card shows the maximum of all currently available temperature sensors, not necessarily CPU Package.
- **Affinity changes are temporary.** They do not survive a process restart. Applications may change their own affinity, and launchers may delegate work to another process. Multi-processor-group affinity changes are not supported.
- **Protected processes may deny access.** Before process operations, CoreScope checks creation time to guard against PID reuse.

## Build and test

```powershell
cargo run --release
cargo test
cargo fmt --check
node --test scripts/ui-test.cjs
powershell -NoProfile -File scripts/build.ps1
powershell -NoProfile -File scripts/build-installer.ps1
```

Building the installer requires Inno Setup 6.1 or later. Set `ISCC` to the compiler path if it is not found automatically.

For API development, run `corescope.exe --no-open --port 8899`. The desktop application normally chooses a free local port.

```powershell
powershell -NoProfile -File scripts/smoke.ps1 -Base http://127.0.0.1:8899
powershell -NoProfile -File scripts/sensors-smoke.ps1 -Base http://127.0.0.1:8899
```

`scripts/activity-smoke.ps1` checks a real CPU-bound process pinned first to CPU 0 and then CPU 1, activity expiry, and time accounting. It requires an elevated CoreScope instance and two logical CPUs.

The API smoke test covers process identity, memory queries, affinity validation, launch behavior, and request guards. The sensor smoke test requires a machine exposing supported Windows and extended sensors; it checks real readings, history growth, and independent collector operation.

| File | Responsibility |
| --- | --- |
| `src/activity.rs` | ETW session lifecycle and per-CPU execution intervals |
| `src/platform.rs` | Windows process handles, affinity, launch, and virtual memory |
| `src/main.rs` | Collectors, snapshots, and local API |
| `src/desktop.rs` | Native window, tray, DPI, and application lifecycle |
| `src/webview.rs` | Embedded WebView2 host |
| `assets/index.html`, `assets/i18n.js` | Interface and localization |
| `assets/sensors.ps1` | Windows and integrated-library sensor collection |

The server listens only on `127.0.0.1`, checks Host/Origin, and requires a random session token for changes. This protects against cross-origin requests, not other programs running as the same user. Do not expose it through a reverse proxy.

## License

CoreScope is licensed under [MIT](LICENSE). Bundled LibreHardwareMonitorLib is licensed under MPL-2.0; other dependencies retain their respective licenses. See [`assets/hardware/SOURCE.txt`](assets/hardware/SOURCE.txt), the accompanying license notices, and [`assets/webview2/LICENSE.txt`](assets/webview2/LICENSE.txt).
