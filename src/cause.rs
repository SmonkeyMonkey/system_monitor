use sysinfo::{Pid, System};

pub struct Suspect {
    pub name: String,
    pub cpu_usage: f32,
}

pub struct MemoryConsumer {
    pub name: String,
    pub memory_bytes: u64,
    pub pid: i32,
}

// When a component is running hot, the most likely explanation on a
// consumer machine is whichever processes are currently burning the most
// CPU. This is a heuristic, not a causal proof (e.g. thermal paste aging,
// clogged fans, or ambient temp won't show up here).
pub fn top_cpu_consumers(sys: &System, limit: usize) -> Vec<Suspect> {
    let mut processes: Vec<Suspect> = sys
        .processes()
        .values()
        .map(|p| Suspect {
            name: p.name().to_string_lossy().to_string(),
            cpu_usage: p.cpu_usage(),
        })
        .filter(|s| s.cpu_usage > 1.0)
        .collect();

    processes.sort_by(|a, b| b.cpu_usage.total_cmp(&a.cpu_usage));
    processes.truncate(limit);
    processes
}

pub fn top_memory_consumers(sys: &System, limit: usize) -> Vec<MemoryConsumer> {
    let mut processes: Vec<MemoryConsumer> = sys
        .processes()
        .iter()
        .map(|(&pid, p)| MemoryConsumer {
            name: p.name().to_string_lossy().to_string(),
            memory_bytes: p.memory(),
            pid: pid.as_u32() as i32,
        })
        .filter(|p| p.memory_bytes > 0)
        .collect();

    processes.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    processes.truncate(limit);
    processes
}

pub struct MemoryProcessGroup {
    pub app_name: String,
    pub total_memory_bytes: u64,
    pub children: Vec<MemoryConsumer>,
    pub app_bundle_path: Option<String>,
}

pub struct CpuConsumer {
    pub name: String,
    pub cpu_usage: f32,
    pub pid: i32,
}

pub struct CpuProcessGroup {
    pub app_name: String,
    pub total_cpu_usage: f32,
    pub children: Vec<CpuConsumer>,
    pub app_bundle_path: Option<String>,
}

// Multi-process apps nest their helper .app bundles inside the main one
// (e.g. ".../Google Chrome.app/Contents/Frameworks/.../Helpers/Google
// Chrome Helper.app/Contents/MacOS/Google Chrome Helper"), so the
// outermost ".app" segment in the executable path is the one whose icon
// represents the whole app, not the innermost helper bundle.
fn outermost_app_bundle(exe_path: &std::path::Path) -> Option<String> {
    let path_str = exe_path.to_str()?;
    let idx = path_str.find(".app/").or_else(|| {
        if path_str.ends_with(".app") {
            Some(path_str.len() - 4)
        } else {
            None
        }
    })?;
    Some(path_str[..idx + 4].to_string())
}

fn app_name_from_bundle(bundle_path: &str) -> String {
    std::path::Path::new(bundle_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| bundle_path.to_string())
}

struct ProcessOwner {
    label: String,
    bundle_path: Option<String>,
}

// Determines which app a process "belongs to" for grouping. Three
// fallbacks, most to least precise:
//
// 1. macOS process coalitions (via `lsappinfo`, macOS-only): the same
//    mechanism Activity Monitor uses. This is the only thing that
//    correctly attributes XPC-spawned processes like Safari's per-tab
//    com.apple.WebKit.WebContent, which launchd owns at the OS level
//    (PPID 1) with no visible link back to Safari.
// 2. Walking up the real parent-process chain to the nearest ancestor
//    (including self) whose executable resolves to an .app bundle. Covers
//    Chrome/Electron-style apps that fork their own helpers directly, on
//    any OS, when the coalition map (macOS-only) has nothing for a PID.
// 3. The process's own name, when neither of the above resolves anything
//    (bare CLI tools, some system daemons).
fn find_owner(
    sys: &System,
    pid: Pid,
    #[cfg(target_os = "macos")] coalitions: &std::collections::HashMap<
        u32,
        crate::app_coalitions::AppInfo,
    >,
    bundle_by_pid: &std::collections::HashMap<Pid, String>,
) -> ProcessOwner {
    #[cfg(target_os = "macos")]
    if let Some(info) = coalitions.get(&pid.as_u32()) {
        return ProcessOwner {
            label: info.app_name.clone(),
            bundle_path: info.bundle_path.clone(),
        };
    }

    let mut current = pid;
    let mut hops = 0;
    // Bounded walk: process trees are shallow in practice, and this
    // guards against any accidental cycle in parent links.
    while hops < 32 {
        if let Some(bundle_path) = bundle_by_pid.get(&current) {
            let label = app_name_from_bundle(bundle_path);
            return ProcessOwner {
                label,
                bundle_path: Some(bundle_path.clone()),
            };
        }
        match sys.process(current).and_then(|p| p.parent()) {
            Some(parent) if parent != current => {
                current = parent;
                hops += 1;
            }
            _ => break,
        }
    }

    let label = sys
        .process(pid)
        .map(|p| p.name().to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    ProcessOwner {
        label,
        bundle_path: None,
    }
}

// Precomputes, for every process that itself resolves to an app bundle,
// its bundle path — this is the set of "possible owners" `find_owner`
// walks up the parent chain looking for (fallback 2 above).
fn bundle_paths_by_pid(sys: &System) -> std::collections::HashMap<Pid, String> {
    sys.processes()
        .iter()
        .filter_map(|(&pid, p)| p.exe().and_then(outermost_app_bundle).map(|b| (pid, b)))
        .collect()
}

pub fn grouped_memory_consumers(sys: &System) -> Vec<MemoryProcessGroup> {
    use std::collections::BTreeMap;

    #[cfg(target_os = "macos")]
    let coalitions = crate::app_coalitions::pid_to_app_map();
    let bundle_by_pid = bundle_paths_by_pid(sys);
    let mut groups: BTreeMap<String, Vec<MemoryConsumer>> = BTreeMap::new();
    let mut bundle_paths: BTreeMap<String, String> = BTreeMap::new();

    for (&pid, p) in sys.processes() {
        let memory_bytes = p.memory();
        if memory_bytes == 0 {
            continue;
        }
        let name = p.name().to_string_lossy().to_string();
        let owner = find_owner(
            sys,
            pid,
            #[cfg(target_os = "macos")]
            &coalitions,
            &bundle_by_pid,
        );

        if let Some(bundle_path) = owner.bundle_path {
            bundle_paths
                .entry(owner.label.clone())
                .or_insert(bundle_path);
        }

        groups.entry(owner.label).or_default().push(MemoryConsumer {
            name,
            memory_bytes,
            pid: pid.as_u32() as i32,
        });
    }

    let mut result: Vec<MemoryProcessGroup> = groups
        .into_iter()
        .map(|(app_name, mut children)| {
            children.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
            let total_memory_bytes = children.iter().map(|c| c.memory_bytes).sum();
            let app_bundle_path = bundle_paths.remove(&app_name);
            MemoryProcessGroup {
                app_name,
                total_memory_bytes,
                children,
                app_bundle_path,
            }
        })
        .collect();

    result.sort_by(|a, b| b.total_memory_bytes.cmp(&a.total_memory_bytes));
    result
}

pub fn grouped_cpu_consumers(sys: &System) -> Vec<CpuProcessGroup> {
    use std::collections::BTreeMap;

    #[cfg(target_os = "macos")]
    let coalitions = crate::app_coalitions::pid_to_app_map();
    let bundle_by_pid = bundle_paths_by_pid(sys);
    let mut groups: BTreeMap<String, Vec<CpuConsumer>> = BTreeMap::new();
    let mut bundle_paths: BTreeMap<String, String> = BTreeMap::new();

    for (&pid, p) in sys.processes() {
        let cpu_usage = p.cpu_usage();
        if cpu_usage <= 0.0 {
            continue;
        }
        let name = p.name().to_string_lossy().to_string();
        let owner = find_owner(
            sys,
            pid,
            #[cfg(target_os = "macos")]
            &coalitions,
            &bundle_by_pid,
        );

        if let Some(bundle_path) = owner.bundle_path {
            bundle_paths
                .entry(owner.label.clone())
                .or_insert(bundle_path);
        }

        groups.entry(owner.label).or_default().push(CpuConsumer {
            name,
            cpu_usage,
            pid: pid.as_u32() as i32,
        });
    }

    let mut result: Vec<CpuProcessGroup> = groups
        .into_iter()
        .map(|(app_name, mut children)| {
            children.sort_by(|a, b| b.cpu_usage.total_cmp(&a.cpu_usage));
            let total_cpu_usage = children.iter().map(|c| c.cpu_usage).sum();
            let app_bundle_path = bundle_paths.remove(&app_name);
            CpuProcessGroup {
                app_name,
                total_cpu_usage,
                children,
                app_bundle_path,
            }
        })
        .collect();

    result.sort_by(|a, b| b.total_cpu_usage.total_cmp(&a.total_cpu_usage));
    result
}
