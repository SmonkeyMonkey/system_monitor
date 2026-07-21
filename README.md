# System Monitor

A macOS system monitor written in Rust — lives in the menu bar, tracks CPU/GPU/battery temperatures against your machine's own history, and shows CPU, memory, and storage usage grouped by app rather than by raw process name.

![menu bar icon](https://img.shields.io/badge/platform-macOS-blue)

## What it does

- **Menu bar temperature indicator** — shows the hottest sensor reading (e.g. `🌡 62°C`) next to the clock, refreshed every 10s. Flags a sensor with `⚠️` when it's running hotter than your machine's own historical baseline (not a generic threshold).
- **System Monitor window** — three tabs:
  - **Overview** — CPU / Memory / hottest-sensor rings, per-core usage, uptime.
  - **Memory** / **CPU** — processes grouped by the app that owns them (e.g. every Chrome tab process collapses under "Google Chrome", every Safari `WebContent` process collapses under "Safari") rather than as dozens of identical-looking rows. Double-click a process for its real memory footprint (the same figure Activity Monitor's Memory column is based on).
- **Storage window** — two tabs:
  - **Browse** — full disk scan, largest files/folders first, expandable tree, right-click → Show in Finder.
  - **Caches** — every detected build/dependency cache directory found during the scan (`target`, `node_modules`, `.venv`, `__pycache__`, `.gradle`, `Pods`, `DerivedData`, ...), with a project column so you know what each one belongs to. Check the ones you don't need and move them to the Trash — only the cache folder is removed, never the project itself.
- **Launch at Login** — one click, backed by a real `launchd` LaunchAgent (survives reboots, restarts itself if it crashes).

## Requirements

- macOS (Apple Silicon or Intel)
- [Rust](https://rustup.rs/) (stable toolchain; install via `rustup` if you don't have it)

No other setup needed — reading temperature sensors and process info doesn't require any special permission on macOS.

## Building

```bash
git clone <this-repo>
cd system_monitor
cargo build --release
```

This produces two binaries in `target/release/`:

| Binary | What it is |
|---|---|
| `menubar` | The menu bar app — this is the one most people want. |
| `system_monitor` | A one-shot CLI that prints current sensor readings to the terminal and exits. Useful for scripting or a quick check without opening the GUI. |

## Running

**Menu bar app** (the GUI):

```bash
./target/release/menubar
```

A thermometer icon (`🌡`) appears in the menu bar. Click it for the dropdown menu — **System Monitor…**, **Storage…**, **Launch at Login**, and **Quit**.

To have it start automatically every time you log in, click **Launch at Login** in the menu (or run it once and toggle the checkbox) — this installs a per-user `launchd` agent at `~/Library/LaunchAgents/com.system-monitor.menubar.plist`. No `sudo` required. Toggle it again to uninstall.

To run it in the background just for the current session, without installing the LaunchAgent:

```bash
nohup ./target/release/menubar >/dev/null 2>&1 &
disown
```

(A bare `menubar &` also backgrounds it, but it's still tied to the terminal session and quits if the terminal closes; `nohup` + `disown` detaches it so it keeps running after the terminal closes. Don't use `open` here — it's for `.app` bundles and documents, and on a bare executable like this it hands off to a default file-opener app instead of running it.)

**One-shot CLI**:

```bash
./target/release/system_monitor
```

Prints a snapshot of every readable sensor with your personal baseline (once you've run it a few times) or a reference "normal" range, and exits.

## How the temperature baseline works

The first few times you run it, there's no history yet, so readings are compared against static reference ranges per component type (CPU, GPU, SSD, battery). After \~5 samples for a given sensor, it switches to comparing against *your* machine's own mean and standard deviation for that sensor — a reading is flagged as hot only when it's meaningfully above what's normal for your specific hardware, not some generic number that might not fit a given model, cooling setup, or ambient temperature. History is stored at `~/.system_monitor/history.json`.

## Project layout

```
src/
  lib.rs              — Collector: reads sensors/CPU/memory into a Snapshot
  baseline.rs          — per-sensor historical mean/stddev tracking
  reference.rs          — static fallback temperature ranges
  cause.rs              — groups processes by owning app (memory/CPU)
  app_coalitions.rs      — macOS process-coalition lookup (via `lsappinfo`),
                          the mechanism that correctly attributes Safari's
                          per-tab WebContent processes etc. to their real app
  storage.rs            — disk scanning, largest-entries tree, cache detection
  launchd.rs            — install/uninstall the LaunchAgent
  main.rs               — one-shot CLI binary
  bin/menubar.rs         — the menu bar app (AppKit via objc2)
  bin/menubar_support/   — AppKit data sources/delegates the menu bar app uses
```

## Platform note

This is macOS-only. It's built directly against AppKit (via the `objc2`/`objc2-app-kit` crates), and several features — process coalitions via `lsappinfo`, the LaunchAgent installer, the memory-footprint lookup via `proc_pid_rusage` — are macOS-specific APIs with no cross-platform equivalent.
