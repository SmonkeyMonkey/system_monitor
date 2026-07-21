use std::path::{Path, PathBuf};
use sysinfo::Disks;

pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
}

// Modern macOS splits every physical volume into a read-only System
// volume (mounted at /) and a writable Data volume (mounted at
// /System/Volumes/Data), joined by firmlinks so they look like one
// filesystem to the user — but sysinfo enumerates them as two separate
// mounts, which would otherwise show up as two "disks" for what's
// actually one drive. The System volume is never where space is actually
// used (nothing writes to it), so it's filtered out here.
pub fn list_disks() -> Vec<DiskInfo> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| !d.is_read_only())
        .map(|d| DiskInfo {
            name: d.name().to_string_lossy().to_string(),
            mount_point: d.mount_point().to_string_lossy().to_string(),
            total_bytes: d.total_space(),
            available_bytes: d.available_space(),
            is_removable: d.is_removable(),
        })
        .collect()
}

// A directory's full contents are captured (not just its total size) so
// the UI can lazily expand any nested folder — including ones far below
// the top-level list — using data this single scan already has, instead
// of re-walking the disk from scratch every time a row is expanded.
pub struct Entry {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub children: Option<Vec<Entry>>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.children.is_some()
    }
}

// Reports how many filesystem entries have been visited so far, for a
// "Scanning... 1m 23s, 45,000 items" style progress indicator — there's
// no way to know the total up front (that's what the scan is computing),
// so elapsed count is all that's meaningful here.
pub struct ScanProgress {
    pub items_visited: u64,
}

// Recursively scans `root` and every directory beneath it, computing
// each directory's total size and recording its full children list along
// the way — the whole subtree is captured once, up front, so expanding
// any row later in the UI (see storage_outline.rs) is just reading
// already-scanned data rather than hitting the disk again. Unreadable
// directories (permission denied — common under / without Full Disk
// Access) are silently skipped rather than failing the whole scan.
// `on_progress` is called periodically (not on every single file, to
// avoid flooding the reporting channel) so the caller can show a live
// counter.
pub fn scan_largest_entries(root: &Path, on_progress: impl Fn(ScanProgress)) -> Vec<Entry> {
    let mut visited: u64 = 0;
    let mut report = |visited: u64| {
        if visited.is_multiple_of(500) {
            on_progress(ScanProgress {
                items_visited: visited,
            });
        }
    };

    let mut entries = scan_dir(root, &mut visited, &mut report);
    entries.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    entries
}

// Directory names recognized as build/dependency caches: safe to delete
// entirely (the owning project regenerates them on the next
// build/install), as opposed to source files or data the project can't
// recreate on its own.
const CACHE_DIR_NAMES: &[&str] = &[
    "target",       // Rust (cargo)
    "node_modules", // JS/TS (npm/yarn/pnpm)
    ".venv",
    "venv",
    "__pycache__", // Python
    ".next",       // Next.js
    ".nuxt",       // Nuxt
    "dist",
    "build",         // generic JS/web build output
    ".gradle",       // Gradle (Java/Kotlin/Android)
    "Pods",          // CocoaPods (iOS/macOS)
    "DerivedData",   // Xcode
    ".pytest_cache", // Python test cache
    ".tox",          // Python tox
                     // Deliberately not included: ".cache". Unlike the entries above,
                     // ~/.cache (and app-specific "*/.cache" folders) is a general-purpose
                     // cache location many tools share, not something one specific
                     // project regenerates on its next build — deleting it can be lossy
                     // for some apps in ways target/node_modules never are.
];

#[derive(Clone)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub size_bytes: u64,
    // The directory that contains this cache dir — e.g. for
    // .../myapp/target, this is .../myapp. Shown as the "Project" column
    // so the user can tell which project a cache belongs to.
    pub project_path: PathBuf,
}

// Walks an already-scanned tree (see scan_largest_entries) looking for
// directories whose name matches a known build/dependency cache. Doesn't
// recurse into a match's children — a node_modules or target directory
// is treated as one deletable unit, not searched for caches-within-caches
// (which would just be its own internal structure, not another project's
// cache).
pub fn find_build_caches(entries: &[Entry]) -> Vec<CacheEntry> {
    let mut results = Vec::new();
    for entry in entries {
        collect_caches(entry, &mut results);
    }
    results.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    results
}

fn collect_caches(entry: &Entry, results: &mut Vec<CacheEntry>) {
    let Some(children) = &entry.children else {
        return;
    };

    let name = entry
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if CACHE_DIR_NAMES.contains(&name.as_str()) {
        let project_path = entry
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| entry.path.clone());
        results.push(CacheEntry {
            path: entry.path.clone(),
            size_bytes: entry.size_bytes,
            project_path,
        });
        return;
    }

    for child in children {
        collect_caches(child, results);
    }
}

fn scan_dir(dir: &Path, visited: &mut u64, report: &mut impl FnMut(u64)) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return entries;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_symlink() {
            continue;
        }
        *visited += 1;
        report(*visited);

        if metadata.is_dir() {
            let mut children = scan_dir(&path, visited, report);
            children.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
            let size_bytes = children.iter().map(|c| c.size_bytes).sum();
            entries.push(Entry {
                path,
                size_bytes,
                children: Some(children),
            });
        } else {
            entries.push(Entry {
                path,
                size_bytes: metadata.len(),
                children: None,
            });
        }
    }
    entries
}
