// macOS menu bar app: shows the hottest sensor reading (e.g. "🌡 62°C")
// next to the battery/wifi icons, refreshed on a timer. Reads sensors
// directly in-process — no separate collector process or network hop.

#![cfg(target_os = "macos")]

#[path = "menubar_support/caches_table.rs"]
mod caches_table;
#[path = "menubar_support/collector_worker.rs"]
mod collector_worker;
#[path = "menubar_support/process_outline.rs"]
mod process_outline;
#[path = "menubar_support/process_rusage.rs"]
mod process_rusage;
#[path = "menubar_support/ring_view.rs"]
mod ring_view;
#[path = "menubar_support/storage_outline.rs"]
mod storage_outline;

use caches_table::CachesTableDataSource;
use collector_worker::CollectorWorker;
use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSFont,
    NSMenu, NSMenuItem, NSOutlineView, NSScrollView, NSStatusBar, NSStatusBarButton, NSStatusItem,
    NSTabView, NSTabViewItem, NSTableColumn, NSTableView, NSTextAlignment, NSTextField,
    NSVariableStatusItemLength, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
    ns_string,
};
use process_outline::ProcessOutlineDataSource;
use ring_view::RingView;
use std::cell::RefCell;
use storage_outline::StorageOutlineDataSource;

struct InfoWindowWidgets {
    cpu_ring: Retained<RingView>,
    cpu_ring_label: Retained<NSTextField>,
    ram_ring: Retained<RingView>,
    ram_ring_label: Retained<NSTextField>,
    temp_ring: Retained<RingView>,
    temp_ring_label: Retained<NSTextField>,
    detail_label: Retained<NSTextField>,
    memory_ring: Retained<RingView>,
    memory_ring_label: Retained<NSTextField>,
    memory_summary_label: Retained<NSTextField>,
    memory_outline: Retained<NSOutlineView>,
    memory_data_source: Retained<ProcessOutlineDataSource>,
    cpu_tab_ring: Retained<RingView>,
    cpu_tab_ring_label: Retained<NSTextField>,
    cpu_summary_label: Retained<NSTextField>,
    cpu_outline: Retained<NSOutlineView>,
    cpu_data_source: Retained<ProcessOutlineDataSource>,
}

struct StorageWindowWidgets {
    disk_summary_label: Retained<NSTextField>,
    storage_scan_button: Retained<NSButton>,
    scan_progress_label: Retained<NSTextField>,
    storage_outline: Retained<NSOutlineView>,
    storage_data_source: Retained<StorageOutlineDataSource>,
    caches_table: Retained<NSTableView>,
    caches_data_source: Retained<CachesTableDataSource>,
    caches_summary_label: Retained<NSTextField>,
}

struct State {
    worker: CollectorWorker,
    // The most recently received snapshot, kept around so callers that
    // need one *right now* (menu bar title, window contents) have
    // something to render even before a freshly requested one arrives —
    // it's only ever a poll interval or two stale.
    latest_snapshot: Option<system_monitor::Snapshot>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "SystemMonitorAppTarget"]
    struct AppTarget;

    unsafe impl NSObjectProtocol for AppTarget {}

    impl AppTarget {
        #[unsafe(method(requestSnapshot:))]
        fn request_snapshot(&self, _timer: &objc2_foundation::NSTimer) {
            request_snapshot();
        }

        #[unsafe(method(pollSnapshot:))]
        fn poll_snapshot(&self, _timer: &objc2_foundation::NSTimer) {
            poll_snapshot();
        }

        #[unsafe(method(showInfoWindow:))]
        fn show_info_window(&self, _sender: &objc2::runtime::AnyObject) {
            show_info_window();
        }

        #[unsafe(method(toggleLaunchAtLogin:))]
        fn toggle_launch_at_login(&self, _sender: &objc2::runtime::AnyObject) {
            if system_monitor::launchd::is_installed() {
                system_monitor::launchd::uninstall();
            } else {
                system_monitor::launchd::install();
            }
            request_snapshot();
        }

        #[unsafe(method(showProcessDetails:))]
        fn show_process_details(&self, sender: &objc2_app_kit::NSOutlineView) {
            show_process_details(sender);
        }

        #[unsafe(method(scanForLargeFolders:))]
        fn scan_for_large_folders(&self, _sender: &objc2::runtime::AnyObject) {
            start_storage_scan();
        }

        #[unsafe(method(checkStorageScanProgress:))]
        fn check_storage_scan_progress(&self, _timer: &objc2_foundation::NSTimer) {
            poll_storage_scan();
        }

        #[unsafe(method(showStorageWindow:))]
        fn show_storage_window(&self, _sender: &objc2::runtime::AnyObject) {
            show_storage_window();
        }

        #[unsafe(method(refreshStorageWindow:))]
        fn refresh_storage_window(&self, _timer: &objc2_foundation::NSTimer) {
            refresh_storage_window();
        }

        #[unsafe(method(showInFinder:))]
        fn show_in_finder(&self, _sender: &objc2::runtime::AnyObject) {
            show_selected_entry_in_finder();
        }

        #[unsafe(method(deleteSelectedCaches:))]
        fn delete_selected_caches(&self, _sender: &objc2::runtime::AnyObject) {
            delete_selected_caches();
        }

        #[unsafe(method(showCacheInFinder:))]
        fn show_cache_in_finder(&self, _sender: &objc2::runtime::AnyObject) {
            show_selected_cache_in_finder();
        }

        #[unsafe(method(toggleCacheRow:))]
        fn toggle_cache_row(&self, sender: &objc2_app_kit::NSButton) {
            toggle_cache_row(sender);
        }
    }
);

impl AppTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        unsafe { msg_send![this, init] }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static BUTTON: RefCell<Option<Retained<NSStatusBarButton>>> = const { RefCell::new(None) };
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
    static INFO_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    static INFO_WIDGETS: RefCell<Option<InfoWindowWidgets>> = const { RefCell::new(None) };
    static APP_TARGET: RefCell<Option<Retained<AppTarget>>> = const { RefCell::new(None) };
    static STORAGE_SCAN_RX: RefCell<Option<std::sync::mpsc::Receiver<StorageScanMessage>>> = const { RefCell::new(None) };
    static STORAGE_SCAN_STARTED_AT: RefCell<Option<std::time::Instant>> = const { RefCell::new(None) };
    static STORAGE_SCAN_TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    static STORAGE_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    static STORAGE_WIDGETS: RefCell<Option<StorageWindowWidgets>> = const { RefCell::new(None) };
    static STORAGE_REFRESH_TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    // Holds the most recent scan's detected caches so the Caches tab has
    // something to show even if it's opened/rebuilt after the scan
    // finished (the window itself is rebuilt each time it's opened, but
    // the underlying scan data shouldn't need re-scanning the disk).
    static CACHE_ENTRIES: RefCell<Vec<system_monitor::storage::CacheEntry>> = const { RefCell::new(Vec::new()) };
}

// Fire-and-forget: asks the CollectorWorker's background thread for a
// fresh snapshot. Never blocks — the result shows up later via
// `poll_snapshot`, which is what actually updates the UI.
fn request_snapshot() {
    STATE.with(|state| {
        if let Some(state) = state.borrow().as_ref() {
            state.worker.request_snapshot();
        }
    });
}

// Runs on a short-interval main-thread timer. Cheap no-op when the worker
// hasn't produced anything new yet; when it has, updates the menu bar
// title/menu and (if open) the System Monitor window from it.
fn poll_snapshot() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");

    let snapshot = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut()?;
        if let Some(snapshot) = state.worker.try_recv_snapshot() {
            state.latest_snapshot = Some(snapshot);
        }
        state.latest_snapshot.clone()
    });

    let Some(snapshot) = snapshot else { return };

    let title = match snapshot
        .readings
        .iter()
        .max_by(|a, b| a.temperature.total_cmp(&b.temperature))
    {
        Some(hottest) => {
            let flag = if hottest.is_hot { " \u{26A0}" } else { "" };
            format!("\u{1F321} {:.0}\u{00B0}C{flag}", hottest.temperature)
        }
        None => "\u{1F321} --".to_string(),
    };
    let menu = build_menu(&snapshot, mtm);

    BUTTON.with(|button| {
        if let Some(button) = button.borrow().as_ref() {
            button.setTitle(&NSString::from_str(&title));
        }
    });

    STATUS_ITEM.with(|status_item| {
        if let Some(status_item) = status_item.borrow().as_ref() {
            status_item.setMenu(Some(&menu));
        }
    });

    update_info_window(&snapshot);
}

fn build_menu(snapshot: &system_monitor::Snapshot, mtm: MainThreadMarker) -> Retained<NSMenu> {
    use system_monitor::reference::Category;

    let menu = NSMenu::new(mtm);

    let mut groups: std::collections::BTreeMap<Category, Vec<&system_monitor::Reading>> =
        std::collections::BTreeMap::new();
    for reading in &snapshot.readings {
        groups
            .entry(system_monitor::reference::category(&reading.label))
            .or_default()
            .push(reading);
    }

    if groups.is_empty() {
        add_disabled_item(&menu, mtm, "No sensors readable");
    } else {
        for (category, readings) in &groups {
            let avg = readings.iter().map(|r| r.temperature).sum::<f32>() / readings.len() as f32;
            // "now" is the hottest sensor in this category at this instant;
            // "recent max" is the highest value seen across each sensor's
            // retained history window (not necessarily from right now).
            let now = readings
                .iter()
                .map(|r| r.temperature)
                .fold(f32::MIN, f32::max);
            let recent_max = readings
                .iter()
                .filter_map(|r| r.recent_max)
                .fold(f32::MIN, f32::max);
            let any_hot = readings.iter().any(|r| r.is_hot);
            let flag = if any_hot { " \u{26A0}" } else { "" };
            let recent_max_text = if recent_max > f32::MIN {
                format!("{recent_max:.0}\u{00B0}C")
            } else {
                "n/a".to_string()
            };
            let text = format!(
                "{}: avg {:.0}\u{00B0}C, now {:.0}\u{00B0}C, recent max {} ({} sensors){flag}",
                category.label(),
                avg,
                now,
                recent_max_text,
                readings.len()
            );
            add_disabled_item(&menu, mtm, &text);
        }
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_disabled_item(&menu, mtm, "Top CPU consumers");

    if snapshot.top_processes.is_empty() {
        add_disabled_item(&menu, mtm, "  (none above 1% CPU)");
    } else {
        for proc in &snapshot.top_processes {
            let text = format!("  {} \u{2014} {:.1}%", proc.name, proc.cpu_usage);
            add_disabled_item(&menu, mtm, &text);
        }
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let launch_at_login_item = NSMenuItem::new(mtm);
    launch_at_login_item.setTitle(ns_string!("Launch at Login"));
    let launch_state = if system_monitor::launchd::is_installed() {
        objc2_app_kit::NSControlStateValueOn
    } else {
        objc2_app_kit::NSControlStateValueOff
    };
    launch_at_login_item.setState(launch_state);
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe {
                launch_at_login_item.setTarget(Some(target));
                launch_at_login_item.setAction(Some(sel!(toggleLaunchAtLogin:)));
            }
        }
    });
    menu.addItem(&launch_at_login_item);

    let info_item = NSMenuItem::new(mtm);
    info_item.setTitle(ns_string!("System Monitor\u{2026}"));
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe {
                info_item.setTarget(Some(target));
                info_item.setAction(Some(sel!(showInfoWindow:)));
            }
        }
    });
    menu.addItem(&info_item);

    let storage_item = NSMenuItem::new(mtm);
    storage_item.setTitle(ns_string!("Storage\u{2026}"));
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe {
                storage_item.setTarget(Some(target));
                storage_item.setAction(Some(sel!(showStorageWindow:)));
            }
        }
    });
    menu.addItem(&storage_item);

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit_item = NSMenuItem::new(mtm);
    quit_item.setTitle(ns_string!("Quit"));
    unsafe {
        quit_item.setTarget(Some(&NSApplication::sharedApplication(mtm)));
        quit_item.setAction(Some(sel!(terminate:)));
    }
    menu.addItem(&quit_item);

    menu
}

fn add_disabled_item(menu: &NSMenu, mtm: MainThreadMarker, text: &str) {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(text));
    item.setEnabled(false);
    menu.addItem(&item);
}

fn format_detail_text(snapshot: &system_monitor::Snapshot) -> String {
    let stats = &snapshot.system_stats;
    let uptime_h = stats.uptime_secs / 3600;
    let uptime_m = (stats.uptime_secs % 3600) / 60;

    let mut text = String::from("Per-core usage:\n");
    for (i, usage) in stats.per_core_usage.iter().enumerate() {
        text.push_str(&format!("  core {i}: {usage:.1}%\n"));
    }

    text.push_str(&format!(
        "\nUptime: {uptime_h}h {uptime_m}m\n\nTop CPU consumers:\n"
    ));

    if snapshot.top_processes.is_empty() {
        text.push_str("  (none above 1% CPU)\n");
    } else {
        for proc in &snapshot.top_processes {
            text.push_str(&format!(
                "  {} \u{2014} {:.1}%\n",
                proc.name, proc.cpu_usage
            ));
        }
    }

    text
}

fn format_bytes_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

fn format_memory_summary_text(snapshot: &system_monitor::Snapshot) -> String {
    let stats = &snapshot.system_stats;
    let mem_used_gb = format_bytes_gb(stats.used_memory_bytes);
    let mem_total_gb = format_bytes_gb(stats.total_memory_bytes);
    let swap_used_gb = format_bytes_gb(stats.used_swap_bytes);
    let swap_total_gb = format_bytes_gb(stats.total_swap_bytes);

    let process_count: usize = snapshot
        .memory_groups
        .iter()
        .map(|g| g.children.len())
        .sum();

    format!(
        "Memory: {mem_used_gb:.2} GB / {mem_total_gb:.2} GB\n\
         Swap:   {swap_used_gb:.2} GB / {swap_total_gb:.2} GB\n\n\
         {process_count} processes in {} apps:",
        snapshot.memory_groups.len()
    )
}

fn format_cpu_summary_text(snapshot: &system_monitor::Snapshot) -> String {
    let process_count: usize = snapshot.cpu_groups.iter().map(|g| g.children.len()).sum();

    format!(
        "CPU: {:.1}% overall ({} cores)\n\n\
         {process_count} active processes in {} apps:",
        snapshot.system_stats.global_cpu_usage,
        snapshot.system_stats.per_core_usage.len(),
        snapshot.cpu_groups.len()
    )
}

fn format_disk_summary_text(disks: &[system_monitor::storage::DiskInfo]) -> String {
    if disks.is_empty() {
        return "No disks detected.".to_string();
    }

    let mut text = String::new();
    for disk in disks {
        let total_gb = format_bytes_gb(disk.total_bytes);
        let used_gb = format_bytes_gb(disk.total_bytes.saturating_sub(disk.available_bytes));
        let pct = if disk.total_bytes > 0 {
            (disk.total_bytes - disk.available_bytes) as f64 / disk.total_bytes as f64 * 100.0
        } else {
            0.0
        };
        text.push_str(&format!(
            "{} ({})\n  {used_gb:.1} GB / {total_gb:.1} GB used ({pct:.0}%)\n",
            disk.name, disk.mount_point,
        ));
    }
    text
}

fn update_caches_summary(w: &StorageWindowWidgets) {
    let total_gb = format_bytes_gb(w.caches_data_source.total_size_bytes());
    let checked_gb = format_bytes_gb(w.caches_data_source.checked_total_size_bytes());
    w.caches_summary_label
        .setStringValue(&NSString::from_str(&format!(
            "{total_gb:.2} GB total in build/dependency caches \u{2014} {checked_gb:.2} GB selected"
        )));
}

enum StorageScanMessage {
    Progress(u64),
    Done(Vec<system_monitor::storage::Entry>),
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}m {:02}s", secs / 60, secs % 60)
}

// A full scan of / can take a while, so it runs on a background thread
// and reports progress/results back through a channel; a short-lived
// NSTimer polls that channel from the main thread (AppKit calls are not
// thread-safe, so the scanning thread can't touch the outline view
// directly). There's deliberately no timeout — see the "why not just
// stop after 30s" discussion this replaced: an incomplete scan silently
// under-reports what's actually taking up space, so it runs to
// completion and shows a live counter instead.
fn start_storage_scan() {
    STORAGE_WIDGETS.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            w.storage_scan_button.setEnabled(false);
            w.storage_scan_button
                .setTitle(ns_string!("Scanning\u{2026}"));
            w.scan_progress_label
                .setStringValue(ns_string!("Starting\u{2026}"));
        }
    });

    let (tx, rx) = std::sync::mpsc::channel();
    STORAGE_SCAN_RX.with(|cell| *cell.borrow_mut() = Some(rx));
    STORAGE_SCAN_STARTED_AT.with(|cell| *cell.borrow_mut() = Some(std::time::Instant::now()));

    std::thread::spawn(move || {
        let progress_tx = tx.clone();
        // Scanning from /System/Volumes/Data (the actual writable data
        // volume) rather than / avoids walking the same files twice: on
        // modern macOS, / is the read-only system volume joined to the
        // data volume via firmlinks, so a scan starting at / would visit
        // most of the filesystem through both paths and double-count it.
        let entries = system_monitor::storage::scan_largest_entries(
            std::path::Path::new("/System/Volumes/Data"),
            move |p| {
                let _ = progress_tx.send(StorageScanMessage::Progress(p.items_visited));
            },
        );
        let _ = tx.send(StorageScanMessage::Done(entries));
    });

    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    0.2,
                    target,
                    sel!(checkStorageScanProgress:),
                    None,
                    true,
                )
            };
            STORAGE_SCAN_TIMER.with(|t| *t.borrow_mut() = Some(timer));
        }
    });
}

fn poll_storage_scan() {
    // Drain every pending message so a burst of progress updates (or a
    // progress update immediately followed by Done) doesn't get stuck
    // behind whatever the timer's 0.2s cadence would otherwise leave
    // queued.
    let messages: Vec<StorageScanMessage> = STORAGE_SCAN_RX.with(|cell| {
        let borrow = cell.borrow();
        match borrow.as_ref() {
            Some(rx) => std::iter::from_fn(|| rx.try_recv().ok()).collect(),
            None => Vec::new(),
        }
    });

    let elapsed = STORAGE_SCAN_STARTED_AT
        .with(|cell| *cell.borrow())
        .map(|start| start.elapsed())
        .unwrap_or_default();

    for message in messages {
        match message {
            StorageScanMessage::Progress(items_visited) => {
                STORAGE_WIDGETS.with(|w| {
                    if let Some(w) = w.borrow().as_ref() {
                        w.scan_progress_label
                            .setStringValue(&NSString::from_str(&format!(
                                "Scanning\u{2026} {} \u{2014} {items_visited} items",
                                format_elapsed(elapsed)
                            )));
                    }
                });
            }
            StorageScanMessage::Done(entries) => {
                STORAGE_SCAN_TIMER.with(|t| {
                    if let Some(timer) = t.borrow_mut().take() {
                        timer.invalidate();
                    }
                });
                STORAGE_SCAN_RX.with(|cell| *cell.borrow_mut() = None);
                STORAGE_SCAN_STARTED_AT.with(|cell| *cell.borrow_mut() = None);

                let caches = system_monitor::storage::find_build_caches(&entries);

                STORAGE_WIDGETS.with(|w| {
                    if let Some(w) = w.borrow().as_ref() {
                        w.storage_scan_button.setEnabled(true);
                        w.storage_scan_button
                            .setTitle(ns_string!("Scan for Large Folders"));
                        w.scan_progress_label
                            .setStringValue(&NSString::from_str(&format!(
                                "Last scan took {}",
                                format_elapsed(elapsed)
                            )));
                        w.storage_data_source.set_roots(entries);
                        w.storage_outline.reloadData();

                        w.caches_data_source.set_rows(caches.clone());
                        w.caches_table.reloadData();
                        update_caches_summary(w);
                    }
                });

                CACHE_ENTRIES.with(|cell| *cell.borrow_mut() = caches);
            }
        }
    }
}

// Cleans up the info window's timer/state once its close button has been
// clicked (isVisible() flips false, but the NSWindow itself stays alive
// thanks to setReleasedWhenClosed(false) — see show_info_window). Called
// from poll_snapshot() rather than its own timer now, since there's only
// one shared polling loop.
fn check_info_window_closed() -> bool {
    let window_closed = INFO_WINDOW.with(|w| match w.borrow().as_ref() {
        Some(window) => !window.isVisible(),
        None => true,
    });
    if window_closed {
        INFO_WIDGETS.with(|w| *w.borrow_mut() = None);
        INFO_WINDOW.with(|w| *w.borrow_mut() = None);
    }
    window_closed
}

fn update_info_window(snapshot: &system_monitor::Snapshot) {
    if check_info_window_closed() {
        return;
    }

    let mem_pct = if snapshot.system_stats.total_memory_bytes > 0 {
        snapshot.system_stats.used_memory_bytes as f32
            / snapshot.system_stats.total_memory_bytes as f32
    } else {
        0.0
    };

    let hottest = snapshot
        .readings
        .iter()
        .max_by(|a, b| a.temperature.total_cmp(&b.temperature));
    let temp_fraction = hottest
        .map(|r| (r.temperature / 100.0).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let temp_text = match hottest {
        Some(r) => format!("{:.0}\u{00B0}C", r.temperature),
        None => "n/a".to_string(),
    };

    let detail_text = format_detail_text(snapshot);
    let memory_summary_text = format_memory_summary_text(snapshot);
    let cpu_summary_text = format_cpu_summary_text(snapshot);

    INFO_WIDGETS.with(|widgets| {
        if let Some(w) = widgets.borrow().as_ref() {
            w.cpu_ring
                .set_value(snapshot.system_stats.global_cpu_usage / 100.0);
            w.cpu_ring_label
                .setStringValue(&NSString::from_str(&format!(
                    "{:.0}%",
                    snapshot.system_stats.global_cpu_usage
                )));

            w.ram_ring.set_value(mem_pct);
            w.ram_ring_label
                .setStringValue(&NSString::from_str(&format!("{:.0}%", mem_pct * 100.0)));

            w.temp_ring.set_value(temp_fraction);
            w.temp_ring_label
                .setStringValue(&NSString::from_str(&temp_text));

            w.detail_label
                .setStringValue(&NSString::from_str(&detail_text));

            w.memory_ring.set_value(mem_pct);
            w.memory_ring_label
                .setStringValue(&NSString::from_str(&format!("{:.0}%", mem_pct * 100.0)));
            w.memory_summary_label
                .setStringValue(&NSString::from_str(&memory_summary_text));

            let memory_groups = snapshot
                .memory_groups
                .iter()
                .map(|g| process_outline::Group {
                    app_name: g.app_name.clone(),
                    total_value: g.total_memory_bytes as f64,
                    children: g
                        .children
                        .iter()
                        .map(|c| process_outline::Child {
                            name: c.name.clone(),
                            value: c.memory_bytes as f64,
                            pid: c.pid,
                        })
                        .collect(),
                    app_bundle_path: g.app_bundle_path.clone(),
                })
                .collect();
            w.memory_data_source.set_groups(memory_groups);
            w.memory_outline.reloadData();

            let cpu_groups = snapshot
                .cpu_groups
                .iter()
                .map(|g| process_outline::Group {
                    app_name: g.app_name.clone(),
                    total_value: g.total_cpu_usage as f64,
                    children: g
                        .children
                        .iter()
                        .map(|c| process_outline::Child {
                            name: c.name.clone(),
                            value: c.cpu_usage as f64,
                            pid: c.pid,
                        })
                        .collect(),
                    app_bundle_path: g.app_bundle_path.clone(),
                })
                .collect();
            w.cpu_data_source.set_groups(cpu_groups);
            w.cpu_outline.reloadData();
            w.cpu_summary_label
                .setStringValue(&NSString::from_str(&cpu_summary_text));

            w.cpu_tab_ring
                .set_value(snapshot.system_stats.global_cpu_usage / 100.0);
            w.cpu_tab_ring_label
                .setStringValue(&NSString::from_str(&format!(
                    "{:.0}%",
                    snapshot.system_stats.global_cpu_usage
                )));
        }
    });
}

fn refresh_storage_window() {
    let window_closed = STORAGE_WINDOW.with(|w| match w.borrow().as_ref() {
        Some(window) => !window.isVisible(),
        None => true,
    });
    if window_closed {
        STORAGE_REFRESH_TIMER.with(|t| {
            if let Some(timer) = t.borrow_mut().take() {
                timer.invalidate();
            }
        });
        STORAGE_WIDGETS.with(|w| *w.borrow_mut() = None);
        STORAGE_WINDOW.with(|w| *w.borrow_mut() = None);
        return;
    }

    let disk_summary_text = format_disk_summary_text(&system_monitor::storage::list_disks());
    STORAGE_WIDGETS.with(|widgets| {
        if let Some(w) = widgets.borrow().as_ref() {
            w.disk_summary_label
                .setStringValue(&NSString::from_str(&disk_summary_text));
        }
    });
}

// `position` picks which autoresizing margins keep the widget anchored as
// the window is resized: the left column sticks to the left edge, the
// right column to the right edge, and the middle one splits the
// difference so all three fan out evenly instead of bunching up on the
// left when the window grows.
#[derive(Clone, Copy)]
enum RingPosition {
    Left,
    Middle,
    Right,
}

fn build_ring_widget(
    mtm: MainThreadMarker,
    content_view: &objc2_app_kit::NSView,
    x: f64,
    y: f64,
    diameter: f64,
    title: &str,
    position: RingPosition,
) -> (Retained<RingView>, Retained<NSTextField>) {
    use objc2_app_kit::NSAutoresizingMaskOptions as Mask;

    let column_width = diameter + 20.0;
    let ring_x = x + (column_width - diameter) / 2.0;
    // ViewMinYMargin keeps the widget pinned to the top of its container
    // as the window grows downward; without it AppKit's default is to
    // keep the bottom margin fixed, which drags the ring down instead.
    let autoresizing_mask = Mask::ViewMinYMargin
        | match position {
            RingPosition::Left => Mask::ViewMaxXMargin,
            RingPosition::Middle => Mask::ViewMinXMargin | Mask::ViewMaxXMargin,
            RingPosition::Right => Mask::ViewMinXMargin,
        };

    let title_rect = NSRect::new(
        NSPoint::new(x, y + diameter + 4.0),
        NSSize::new(column_width, 20.0),
    );
    let title_label = NSTextField::alloc(mtm);
    let title_label = NSTextField::initWithFrame(title_label, title_rect);
    title_label.setEditable(false);
    title_label.setBezeled(false);
    title_label.setDrawsBackground(false);
    title_label.setSelectable(false);
    title_label.setAlignment(NSTextAlignment::Center);
    title_label.setStringValue(&NSString::from_str(title));
    title_label.setAutoresizingMask(autoresizing_mask);
    content_view.addSubview(&title_label);

    let ring_rect = NSRect::new(NSPoint::new(ring_x, y), NSSize::new(diameter, diameter));
    let ring = RingView::new(mtm, ring_rect);
    ring.setAutoresizingMask(autoresizing_mask);
    content_view.addSubview(&ring);

    let value_rect = NSRect::new(
        NSPoint::new(x, y + diameter / 2.0 - 10.0),
        NSSize::new(column_width, 20.0),
    );
    let value_label = NSTextField::alloc(mtm);
    let value_label = NSTextField::initWithFrame(value_label, value_rect);
    value_label.setEditable(false);
    value_label.setBezeled(false);
    value_label.setDrawsBackground(false);
    value_label.setAutoresizingMask(autoresizing_mask);
    value_label.setSelectable(false);
    value_label.setAlignment(NSTextAlignment::Center);
    value_label.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
    content_view.addSubview(&value_label);

    (ring, value_label)
}

#[allow(clippy::type_complexity)]
fn build_overview_tab(
    mtm: MainThreadMarker,
    page_rect: NSRect,
) -> (
    Retained<NSView>,
    Retained<RingView>,
    Retained<NSTextField>,
    Retained<RingView>,
    Retained<NSTextField>,
    Retained<RingView>,
    Retained<NSTextField>,
    Retained<NSTextField>,
) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), page_rect);
    let window_width = page_rect.size.width;
    let window_height = page_rect.size.height;

    let ring_diameter = 100.0;
    let ring_y = window_height - ring_diameter - 40.0;
    let ring_spacing = window_width / 3.0;

    let (cpu_ring, cpu_ring_label) = build_ring_widget(
        mtm,
        &view,
        ring_spacing * 0.0,
        ring_y,
        ring_diameter,
        "CPU",
        RingPosition::Left,
    );
    let (ram_ring, ram_ring_label) = build_ring_widget(
        mtm,
        &view,
        ring_spacing * 1.0,
        ring_y,
        ring_diameter,
        "Memory",
        RingPosition::Middle,
    );
    let (temp_ring, temp_ring_label) = build_ring_widget(
        mtm,
        &view,
        ring_spacing * 2.0,
        ring_y,
        ring_diameter,
        "Hottest",
        RingPosition::Right,
    );

    let detail_rect = NSRect::new(
        NSPoint::new(16.0, 16.0),
        NSSize::new(window_width - 32.0, ring_y - 32.0),
    );
    let detail_label = NSTextField::alloc(mtm);
    let detail_label = NSTextField::initWithFrame(detail_label, detail_rect);
    detail_label.setEditable(false);
    detail_label.setBezeled(false);
    detail_label.setDrawsBackground(false);
    detail_label.setSelectable(true);
    detail_label.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    view.addSubview(&detail_label);

    (
        view,
        cpu_ring,
        cpu_ring_label,
        ram_ring,
        ram_ring_label,
        temp_ring,
        temp_ring_label,
        detail_label,
    )
}

fn show_process_details(outline: &NSOutlineView) {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");

    let row = outline.clickedRow();
    if row < 0 {
        return;
    }
    let Some(item) = outline.itemAtRow(row) else {
        return;
    };
    let Some((name, pid)) = process_outline::describe_item(&item) else {
        // Double-clicked an app group header (multiple PIDs, nothing
        // single to report rusage for) rather than a leaf process row.
        return;
    };

    let message = match process_rusage::rusage_for_pid(pid) {
        Some(rusage) => format!(
            "PID: {pid}\n\n\
             Real Memory (RSS): {:.1} MB\n\
             Physical Footprint: {:.1} MB\n\
             Virtual Memory: {:.1} GB\n\n\
             \u{201c}Physical Footprint\u{201d} is the figure Activity Monitor's \
             process list Memory column is based on \u{2014} it can run higher \
             than RSS because it includes compressed and purgeable memory. \
             Virtual Memory reflects the process's whole addressable space, \
             not memory it's actually using.",
            rusage.resident_size as f64 / 1_048_576.0,
            rusage.phys_footprint as f64 / 1_048_576.0,
            rusage.virtual_size as f64 / 1_073_741_824.0,
        ),
        None => format!("PID: {pid}\n\nCouldn't read memory details (process may have exited)."),
    };

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&name));
    alert.setInformativeText(&NSString::from_str(&message));
    alert.runModal();
}

fn show_selected_entry_in_finder() {
    let path = STORAGE_WIDGETS.with(|w| {
        let widgets = w.borrow();
        let widgets = widgets.as_ref()?;
        let row = widgets.storage_outline.clickedRow();
        if row < 0 {
            return None;
        }
        let item = widgets.storage_outline.itemAtRow(row)?;
        storage_outline::path_for_item(&item)
    });

    let Some(path) = path else { return };
    reveal_path_in_finder(&path);
}

fn show_selected_cache_in_finder() {
    let path = STORAGE_WIDGETS.with(|w| {
        let widgets = w.borrow();
        let widgets = widgets.as_ref()?;
        let row = widgets.caches_table.clickedRow();
        widgets.caches_data_source.path_for_row(row)
    });

    let Some(path) = path else { return };
    reveal_path_in_finder(&path);
}

fn reveal_path_in_finder(path: &std::path::Path) {
    let path_str = NSString::from_str(&path.to_string_lossy());
    objc2_app_kit::NSWorkspace::sharedWorkspace()
        .selectFile_inFileViewerRootedAtPath(Some(&path_str), ns_string!(""));
}

fn toggle_cache_row(sender: &objc2_app_kit::NSButton) {
    let row = sender.tag() as objc2_foundation::NSInteger;
    let checked = sender.state() == objc2_app_kit::NSControlStateValueOn;

    STORAGE_WIDGETS.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            w.caches_data_source.set_row_checked(row, checked);
            update_caches_summary(w);
        }
    });
}

fn delete_selected_caches() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");

    let paths: Vec<std::path::PathBuf> = STORAGE_WIDGETS
        .with(|w| {
            w.borrow()
                .as_ref()
                .map(|w| w.caches_data_source.checked_paths())
        })
        .unwrap_or_default();

    if paths.is_empty() {
        return;
    }

    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Move Selected Caches to Trash?"));
    let list = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    alert.setInformativeText(&NSString::from_str(&format!(
        "This moves the following {} folder(s) to the Trash. Only the cache \
         folder itself is removed \u{2014} the project it belongs to is left \
         untouched, and can regenerate the cache on its next build/install.\n\n{list}",
        paths.len()
    )));
    alert.addButtonWithTitle(ns_string!("Move to Trash"));
    alert.addButtonWithTitle(ns_string!("Cancel"));
    // NSAlertFirstButtonReturn == 1000; the buttons above were added in
    // that order, so the first ("Move to Trash") is what we act on.
    if alert.runModal() != 1000 {
        return;
    }

    let urls: Retained<objc2_foundation::NSArray<objc2_foundation::NSURL>> =
        objc2_foundation::NSArray::from_retained_slice(
            &paths
                .iter()
                .map(|p| {
                    objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(
                        &p.to_string_lossy(),
                    ))
                })
                .collect::<Vec<_>>(),
        );

    objc2_app_kit::NSWorkspace::sharedWorkspace().recycleURLs_completionHandler(&urls, None);

    // Optimistically drop the deleted rows now rather than waiting on the
    // completion handler — recycleURLs is effectively synchronous from
    // the Trash's point of view for local files, and wiring up a block2
    // callback just to do the same removal a moment later isn't worth
    // the complexity here.
    STORAGE_WIDGETS.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            w.caches_data_source.remove_checked();
            w.caches_table.reloadData();
            update_caches_summary(w);
        }
    });
}

#[allow(clippy::type_complexity)]
fn build_process_tab(
    mtm: MainThreadMarker,
    page_rect: NSRect,
    ring_title: &str,
    value_column_title: &str,
    metric: process_outline::Metric,
) -> (
    Retained<NSView>,
    Retained<RingView>,
    Retained<NSTextField>,
    Retained<NSTextField>,
    Retained<NSOutlineView>,
    Retained<ProcessOutlineDataSource>,
) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), page_rect);
    let window_width = page_rect.size.width;
    let window_height = page_rect.size.height;

    let ring_diameter = 80.0;
    let ring_y = window_height - ring_diameter - 30.0;

    let (ring, ring_label) = build_ring_widget(
        mtm,
        &view,
        16.0,
        ring_y,
        ring_diameter,
        ring_title,
        RingPosition::Left,
    );

    let summary_rect = NSRect::new(
        NSPoint::new(ring_diameter + 40.0, ring_y + 10.0),
        NSSize::new(window_width - ring_diameter - 56.0, ring_diameter + 20.0),
    );
    let summary_label = NSTextField::alloc(mtm);
    let summary_label = NSTextField::initWithFrame(summary_label, summary_rect);
    summary_label.setEditable(false);
    summary_label.setBezeled(false);
    summary_label.setDrawsBackground(false);
    summary_label.setSelectable(true);
    summary_label.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    view.addSubview(&summary_label);

    let table_rect = NSRect::new(
        NSPoint::new(16.0, 16.0),
        NSSize::new(window_width - 32.0, ring_y - 32.0),
    );
    let scroll_view = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), table_rect);
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    scroll_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let outline = NSOutlineView::initWithFrame(NSOutlineView::alloc(mtm), table_rect);

    let name_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("name"));
    name_column.setTitle(ns_string!("Process"));
    name_column.setWidth(table_rect.size.width * 0.65);
    outline.addTableColumn(&name_column);
    unsafe { outline.setOutlineTableColumn(Some(&name_column)) };

    let value_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("value"));
    value_column.setTitle(&NSString::from_str(value_column_title));
    value_column.setWidth(table_rect.size.width * 0.30);
    outline.addTableColumn(&value_column);

    let data_source = ProcessOutlineDataSource::new(mtm, metric);
    unsafe {
        outline.setDataSource(Some(data_source.as_data_source()));
        outline.setDelegate(Some(data_source.as_delegate()));
        outline.setDoubleAction(Some(sel!(showProcessDetails:)));
    };
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe { outline.setTarget(Some(target)) };
        }
    });

    scroll_view.setDocumentView(Some(&outline));
    view.addSubview(&scroll_view);

    (view, ring, ring_label, summary_label, outline, data_source)
}

#[allow(clippy::type_complexity)]
fn build_storage_tab(
    mtm: MainThreadMarker,
    page_rect: NSRect,
) -> (
    Retained<NSView>,
    Retained<NSTextField>,
    Retained<NSButton>,
    Retained<NSTextField>,
    Retained<NSOutlineView>,
    Retained<StorageOutlineDataSource>,
) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), page_rect);
    let window_width = page_rect.size.width;
    let window_height = page_rect.size.height;

    let summary_height = 90.0;
    let summary_rect = NSRect::new(
        NSPoint::new(16.0, window_height - summary_height - 16.0),
        NSSize::new(window_width - 32.0, summary_height),
    );
    let summary_label = NSTextField::alloc(mtm);
    let summary_label = NSTextField::initWithFrame(summary_label, summary_rect);
    summary_label.setEditable(false);
    summary_label.setBezeled(false);
    summary_label.setDrawsBackground(false);
    summary_label.setSelectable(true);
    summary_label.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    view.addSubview(&summary_label);

    let button_y = summary_rect.origin.y - 36.0;
    let button_rect = NSRect::new(NSPoint::new(16.0, button_y), NSSize::new(220.0, 28.0));
    let scan_button = NSButton::initWithFrame(NSButton::alloc(mtm), button_rect);
    scan_button.setTitle(ns_string!("Scan for Large Folders"));
    scan_button.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
    scan_button.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin);
    unsafe { scan_button.setAction(Some(sel!(scanForLargeFolders:))) };
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe { scan_button.setTarget(Some(target)) };
        }
    });
    view.addSubview(&scan_button);

    let progress_rect = NSRect::new(
        NSPoint::new(246.0, button_y + 4.0),
        NSSize::new(window_width - 246.0 - 16.0, 20.0),
    );
    let progress_label = NSTextField::alloc(mtm);
    let progress_label = NSTextField::initWithFrame(progress_label, progress_rect);
    progress_label.setEditable(false);
    progress_label.setBezeled(false);
    progress_label.setDrawsBackground(false);
    progress_label.setSelectable(false);
    progress_label.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    view.addSubview(&progress_label);

    let table_rect = NSRect::new(
        NSPoint::new(16.0, 16.0),
        NSSize::new(window_width - 32.0, button_y - 32.0),
    );
    let scroll_view = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), table_rect);
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    scroll_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let outline = NSOutlineView::initWithFrame(NSOutlineView::alloc(mtm), table_rect);

    let name_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("name"));
    name_column.setTitle(ns_string!("Name"));
    name_column.setWidth(table_rect.size.width * 0.75);
    outline.addTableColumn(&name_column);
    unsafe { outline.setOutlineTableColumn(Some(&name_column)) };

    let size_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("size"));
    size_column.setTitle(ns_string!("Size"));
    size_column.setWidth(table_rect.size.width * 0.20);
    outline.addTableColumn(&size_column);

    let data_source = StorageOutlineDataSource::new(mtm);
    unsafe { outline.setDataSource(Some(data_source.as_protocol())) };

    let context_menu = NSMenu::new(mtm);
    let show_in_finder_item = NSMenuItem::new(mtm);
    show_in_finder_item.setTitle(ns_string!("Show in Finder"));
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe {
                show_in_finder_item.setTarget(Some(target));
                show_in_finder_item.setAction(Some(sel!(showInFinder:)));
            }
        }
    });
    context_menu.addItem(&show_in_finder_item);
    unsafe { outline.setMenu(Some(&context_menu)) };

    scroll_view.setDocumentView(Some(&outline));
    view.addSubview(&scroll_view);

    (
        view,
        summary_label,
        scan_button,
        progress_label,
        outline,
        data_source,
    )
}

#[allow(clippy::type_complexity)]
fn build_caches_tab(
    mtm: MainThreadMarker,
    page_rect: NSRect,
) -> (
    Retained<NSView>,
    Retained<NSTextField>,
    Retained<NSTableView>,
    Retained<CachesTableDataSource>,
) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), page_rect);
    let window_width = page_rect.size.width;
    let window_height = page_rect.size.height;

    let summary_rect = NSRect::new(
        NSPoint::new(16.0, window_height - 40.0),
        NSSize::new(window_width - 32.0, 20.0),
    );
    let summary_label = NSTextField::alloc(mtm);
    let summary_label = NSTextField::initWithFrame(summary_label, summary_rect);
    summary_label.setEditable(false);
    summary_label.setBezeled(false);
    summary_label.setDrawsBackground(false);
    summary_label.setSelectable(false);
    summary_label.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    view.addSubview(&summary_label);

    let button_rect = NSRect::new(NSPoint::new(16.0, 16.0), NSSize::new(180.0, 28.0));
    let delete_button = NSButton::initWithFrame(NSButton::alloc(mtm), button_rect);
    delete_button.setTitle(ns_string!("Delete Selected"));
    delete_button.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
    // NSView's Y axis grows upward, so "MinYMargin" means the gap *below*
    // the view (near Y=0) is the flexible one — which pins the view to
    // the top, the opposite of what's wanted for a button meant to stay
    // near the window's bottom edge. MaxYMargin (the gap above flexes
    // instead) is what actually keeps it anchored at a fixed distance
    // from the bottom as the window is resized.
    delete_button.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewMaxYMargin);
    unsafe { delete_button.setAction(Some(sel!(deleteSelectedCaches:))) };
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe { delete_button.setTarget(Some(target)) };
        }
    });
    view.addSubview(&delete_button);

    let table_rect = NSRect::new(
        NSPoint::new(16.0, 52.0),
        NSSize::new(window_width - 32.0, window_height - 100.0),
    );
    let scroll_view = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), table_rect);
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    scroll_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let table = NSTableView::initWithFrame(NSTableView::alloc(mtm), table_rect);

    let checkbox_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("checkbox"));
    checkbox_column.setTitle(ns_string!(""));
    checkbox_column.setWidth(28.0);
    table.addTableColumn(&checkbox_column);

    let project_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("project"));
    project_column.setTitle(ns_string!("Project"));
    project_column.setWidth(table_rect.size.width * 0.25);
    table.addTableColumn(&project_column);

    let path_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("path"));
    path_column.setTitle(ns_string!("Path"));
    path_column.setWidth(table_rect.size.width * 0.45);
    table.addTableColumn(&path_column);

    let size_column =
        NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("size"));
    size_column.setTitle(ns_string!("Size"));
    size_column.setWidth(table_rect.size.width * 0.20);
    table.addTableColumn(&size_column);

    let data_source = CachesTableDataSource::new(mtm);
    unsafe {
        table.setDataSource(Some(data_source.as_data_source()));
        table.setDelegate(Some(data_source.as_delegate()));
    };
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            data_source.set_checkbox_target(objc2::rc::Retained::from(target.clone()));
        }
    });

    let context_menu = NSMenu::new(mtm);
    let show_in_finder_item = NSMenuItem::new(mtm);
    show_in_finder_item.setTitle(ns_string!("Show in Finder"));
    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            unsafe {
                show_in_finder_item.setTarget(Some(target));
                show_in_finder_item.setAction(Some(sel!(showCacheInFinder:)));
            }
        }
    });
    context_menu.addItem(&show_in_finder_item);
    unsafe { table.setMenu(Some(&context_menu)) };

    scroll_view.setDocumentView(Some(&table));
    view.addSubview(&scroll_view);

    (view, summary_label, table, data_source)
}

fn show_info_window() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");

    let already_open = INFO_WINDOW.with(|w| w.borrow().is_some());
    if already_open {
        request_snapshot();
        INFO_WINDOW.with(|w| {
            if let Some(window) = w.borrow().as_ref() {
                NSApplication::sharedApplication(mtm).activate();
                window.orderFrontRegardless();
                window.makeKeyWindow();
            }
        });
        return;
    }

    let window_width = 420.0;
    let window_height = 520.0;
    let content_rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(window_width, window_height),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content_rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(ns_string!("System Monitor"));
    window.setMinSize(NSSize::new(window_width, window_height));
    window.center();
    // Without this, clicking the close button lets Cocoa deallocate the
    // NSWindow entirely; our `Retained<NSWindow>` in INFO_WINDOW would
    // then point at freed memory, and the next tick's window/menubar
    // updates would corrupt the process (this is what made the whole
    // menu bar icon disappear after closing the window once).
    unsafe { window.setReleasedWhenClosed(false) };

    let tab_view_rect = content_rect;
    let tab_view = NSTabView::initWithFrame(NSTabView::alloc(mtm), tab_view_rect);
    tab_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    // Set the tab view as the window's content first, then read back its
    // actual contentRect() — see the matching comment in
    // show_storage_window for why guessing this rect (e.g. "window
    // height minus 40") is unreliable.
    window.setContentView(Some(&tab_view));
    let page_rect = tab_view.contentRect();

    let (
        overview_view,
        cpu_ring,
        cpu_ring_label,
        ram_ring,
        ram_ring_label,
        temp_ring,
        temp_ring_label,
        detail_label,
    ) = build_overview_tab(mtm, page_rect);
    overview_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let overview_item = NSTabViewItem::new();
    overview_item.setLabel(ns_string!("Overview"));
    overview_item.setView(Some(&overview_view));
    tab_view.addTabViewItem(&overview_item);

    let (
        memory_view,
        memory_ring,
        memory_ring_label,
        memory_summary_label,
        memory_outline,
        memory_data_source,
    ) = build_process_tab(
        mtm,
        page_rect,
        "Memory Used",
        "Memory",
        process_outline::Metric::Memory,
    );
    memory_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let memory_item = NSTabViewItem::new();
    memory_item.setLabel(ns_string!("Memory"));
    memory_item.setView(Some(&memory_view));
    tab_view.addTabViewItem(&memory_item);

    let (
        cpu_tab_view,
        cpu_tab_ring,
        cpu_tab_ring_label,
        cpu_summary_label,
        cpu_outline,
        cpu_data_source,
    ) = build_process_tab(
        mtm,
        page_rect,
        "CPU Used",
        "CPU",
        process_outline::Metric::Cpu,
    );
    cpu_tab_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let cpu_item = NSTabViewItem::new();
    cpu_item.setLabel(ns_string!("CPU"));
    cpu_item.setView(Some(&cpu_tab_view));
    tab_view.addTabViewItem(&cpu_item);

    NSApplication::sharedApplication(mtm).activate();
    window.orderFrontRegardless();
    window.makeKeyWindow();

    INFO_WINDOW.with(|w| *w.borrow_mut() = Some(window));
    INFO_WIDGETS.with(|w| {
        *w.borrow_mut() = Some(InfoWindowWidgets {
            cpu_ring,
            cpu_ring_label,
            ram_ring,
            ram_ring_label,
            temp_ring,
            temp_ring_label,
            detail_label,
            memory_ring,
            memory_ring_label,
            memory_summary_label,
            memory_outline,
            memory_data_source,
            cpu_tab_ring,
            cpu_tab_ring_label,
            cpu_summary_label,
            cpu_outline,
            cpu_data_source,
        })
    });

    request_snapshot();
    if let Some(snapshot) = STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|s| s.latest_snapshot.clone())
    }) {
        update_info_window(&snapshot);
    }
}

fn show_storage_window() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");

    let already_open = STORAGE_WINDOW.with(|w| w.borrow().is_some());
    if already_open {
        refresh_storage_window();
        STORAGE_WINDOW.with(|w| {
            if let Some(window) = w.borrow().as_ref() {
                NSApplication::sharedApplication(mtm).activate();
                window.orderFrontRegardless();
                window.makeKeyWindow();
            }
        });
        return;
    }

    let window_width = 480.0;
    let window_height = 420.0;
    let content_rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(window_width, window_height),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content_rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(ns_string!("System Monitor \u{2014} Storage"));
    window.setMinSize(NSSize::new(window_width, window_height));
    window.center();
    unsafe { window.setReleasedWhenClosed(false) };

    let tab_view = NSTabView::initWithFrame(NSTabView::alloc(mtm), content_rect);
    tab_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    // Set the tab view as the window's content first, then read back its
    // actual contentRect() — NSTabView insets tab pages by an amount that
    // varies with its style/appearance, and guessing that inset (e.g.
    // "window height minus 40") produced a page rect a bit taller than
    // what AppKit actually gives each tab's view. Any child laid out
    // against the guessed rect that anchors to the bottom (like the
    // "Delete Selected" button, pinned via ViewMinYMargin) ended up
    // placed below the real visible area — invisible until the window
    // was resized and AppKit recomputed layout.
    window.setContentView(Some(&tab_view));
    let page_rect = tab_view.contentRect();

    let (
        storage_view,
        disk_summary_label,
        storage_scan_button,
        scan_progress_label,
        storage_outline,
        storage_data_source,
    ) = build_storage_tab(mtm, page_rect);
    storage_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let browse_item = NSTabViewItem::new();
    browse_item.setLabel(ns_string!("Browse"));
    browse_item.setView(Some(&storage_view));
    tab_view.addTabViewItem(&browse_item);

    let (caches_view, caches_summary_label, caches_table, caches_data_source) =
        build_caches_tab(mtm, page_rect);
    caches_view.setAutoresizingMask(
        objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
            | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let caches_item = NSTabViewItem::new();
    caches_item.setLabel(ns_string!("Caches"));
    caches_item.setView(Some(&caches_view));
    tab_view.addTabViewItem(&caches_item);

    NSApplication::sharedApplication(mtm).activate();
    window.orderFrontRegardless();
    window.makeKeyWindow();

    STORAGE_WINDOW.with(|w| *w.borrow_mut() = Some(window));
    STORAGE_WIDGETS.with(|w| {
        *w.borrow_mut() = Some(StorageWindowWidgets {
            disk_summary_label,
            storage_scan_button,
            scan_progress_label,
            storage_outline,
            storage_data_source,
            caches_table,
            caches_data_source,
            caches_summary_label,
        })
    });

    // Show whatever the last scan (if any) already found, without
    // re-scanning — the Caches tab reuses scan data from the Browse tab
    // rather than triggering its own disk walk.
    let cached_entries = CACHE_ENTRIES.with(|cell| cell.borrow().clone());
    STORAGE_WIDGETS.with(|w| {
        if let Some(w) = w.borrow().as_ref() {
            w.caches_data_source.set_rows(cached_entries);
            w.caches_table.reloadData();
            update_caches_summary(w);
        }
    });

    refresh_storage_window();

    APP_TARGET.with(|target| {
        if let Some(target) = target.borrow().as_ref() {
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    5.0,
                    target,
                    sel!(refreshStorageWindow:),
                    None,
                    true,
                )
            };
            STORAGE_REFRESH_TIMER.with(|t| *t.borrow_mut() = Some(timer));
        }
    });
}

fn main() {
    let mtm = MainThreadMarker::new().expect("menubar must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    STATE.with(|state| {
        *state.borrow_mut() = Some(State {
            worker: CollectorWorker::spawn(),
            latest_snapshot: None,
        });
    });

    let target = AppTarget::new(mtm);
    APP_TARGET.with(|t| *t.borrow_mut() = Some(target.clone()));

    let status_bar = NSStatusBar::systemStatusBar();
    let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
    let button = status_item.button(mtm).expect("status item has a button");
    button.setTitle(ns_string!("\u{1F321} ..."));

    BUTTON.with(|b| *b.borrow_mut() = Some(button));
    STATUS_ITEM.with(|s| *s.borrow_mut() = Some(status_item));

    // Ask for the first snapshot right away so the bar doesn't sit on
    // "..." for 10s; poll_snapshot (below) will pick it up once the
    // background worker thread finishes computing it.
    request_snapshot();

    let _request_timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            10.0,
            &target,
            sel!(requestSnapshot:),
            None,
            true,
        )
    };
    // Polls at a much shorter interval than requests happen, so a fresh
    // snapshot shows up quickly after each request without the request
    // interval itself needing to be short (collect() can take a moment on
    // macOS due to the lsappinfo shell-out).
    let _poll_timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            0.3,
            &target,
            sel!(pollSnapshot:),
            None,
            true,
        )
    };

    app.run();
}
