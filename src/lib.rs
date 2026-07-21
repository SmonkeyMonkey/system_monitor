#[cfg(target_os = "macos")]
pub mod app_coalitions;
pub mod baseline;
pub mod cause;
#[cfg(target_os = "macos")]
pub mod launchd;
pub mod reference;
pub mod storage;

use serde::{Deserialize, Serialize};
use sysinfo::{Components, System};

#[derive(Serialize, Deserialize, Clone)]
pub struct Reading {
    pub label: String,
    pub temperature: f32,
    pub baseline_mean: Option<f32>,
    pub baseline_stddev: Option<f32>,
    pub baseline_samples: usize,
    pub recent_max: Option<f32>,
    pub reference_low: f32,
    pub reference_high: f32,
    pub is_hot: bool,
    pub suspects: Vec<Suspect>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Suspect {
    pub name: String,
    pub cpu_usage: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MemoryConsumer {
    pub name: String,
    pub memory_bytes: u64,
    pub pid: i32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MemoryProcessGroup {
    pub app_name: String,
    pub total_memory_bytes: u64,
    pub children: Vec<MemoryConsumer>,
    pub app_bundle_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CpuConsumer {
    pub name: String,
    pub cpu_usage: f32,
    pub pid: i32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CpuProcessGroup {
    pub app_name: String,
    pub total_cpu_usage: f32,
    pub children: Vec<CpuConsumer>,
    pub app_bundle_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SystemStats {
    pub global_cpu_usage: f32,
    pub per_core_usage: Vec<f32>,
    pub total_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub total_swap_bytes: u64,
    pub used_swap_bytes: u64,
    pub uptime_secs: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Snapshot {
    pub hostname: String,
    pub timestamp_unix: u64,
    pub readings: Vec<Reading>,
    pub top_processes: Vec<Suspect>,
    pub memory_groups: Vec<MemoryProcessGroup>,
    pub cpu_groups: Vec<CpuProcessGroup>,
    pub system_stats: SystemStats,
}

pub struct Collector {
    sys: System,
    components: Components,
    history: baseline::History,
}

impl Collector {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        let components = Components::new_with_refreshed_list();
        sys.refresh_all();
        Self {
            sys,
            components,
            history: baseline::History::load(),
        }
    }

    // Call this once per collection tick. sysinfo needs two refreshes spaced
    // apart to compute real CPU usage deltas, so the very first snapshot
    // after construction will show 0% for every process.
    pub fn collect(&mut self) -> Snapshot {
        self.sys.refresh_all();
        self.components.refresh(true);

        let mut readings = Vec::new();
        for component in self.components.list() {
            let Some(temp) = component.temperature() else {
                continue;
            };
            if !(-40.0..=150.0).contains(&temp) {
                continue;
            }
            let label = component.label().to_string();

            self.history.record(&label, temp);
            let baseline = self.history.baseline_for(&label);
            let reference = reference::lookup(&label);

            let is_hot = match &baseline {
                Some(b) => temp > b.mean + 2.0 * b.stddev,
                None => temp > reference.normal_high,
            };

            let suspects = if is_hot {
                cause::top_cpu_consumers(&self.sys, 3)
                    .into_iter()
                    .map(|s| Suspect {
                        name: s.name,
                        cpu_usage: s.cpu_usage,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            readings.push(Reading {
                label,
                temperature: temp,
                baseline_mean: baseline.as_ref().map(|b| b.mean),
                baseline_stddev: baseline.as_ref().map(|b| b.stddev),
                baseline_samples: baseline.as_ref().map(|b| b.sample_count).unwrap_or(0),
                recent_max: baseline.as_ref().map(|b| b.recent_max),
                reference_low: reference.normal_low,
                reference_high: reference.normal_high,
                is_hot,
                suspects,
            });
        }

        self.history.save();

        let top_processes = cause::top_cpu_consumers(&self.sys, 5)
            .into_iter()
            .map(|s| Suspect {
                name: s.name,
                cpu_usage: s.cpu_usage,
            })
            .collect();

        // Every process here, not just a top-N, since the Memory tab has
        // its own scrollable list rather than a fixed-size menu. Grouped by
        // app so multi-process apps (Chrome, Slack, ...) collapse into one
        // expandable row instead of dozens of "Helper" entries.
        let memory_groups = cause::grouped_memory_consumers(&self.sys)
            .into_iter()
            .map(|g| MemoryProcessGroup {
                app_name: g.app_name,
                total_memory_bytes: g.total_memory_bytes,
                children: g
                    .children
                    .into_iter()
                    .map(|m| MemoryConsumer {
                        name: m.name,
                        memory_bytes: m.memory_bytes,
                        pid: m.pid,
                    })
                    .collect(),
                app_bundle_path: g.app_bundle_path,
            })
            .collect();

        let cpu_groups = cause::grouped_cpu_consumers(&self.sys)
            .into_iter()
            .map(|g| CpuProcessGroup {
                app_name: g.app_name,
                total_cpu_usage: g.total_cpu_usage,
                children: g
                    .children
                    .into_iter()
                    .map(|c| CpuConsumer {
                        name: c.name,
                        cpu_usage: c.cpu_usage,
                        pid: c.pid,
                    })
                    .collect(),
                app_bundle_path: g.app_bundle_path,
            })
            .collect();

        let system_stats = SystemStats {
            global_cpu_usage: self.sys.global_cpu_usage(),
            per_core_usage: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            total_memory_bytes: self.sys.total_memory(),
            used_memory_bytes: self.sys.used_memory(),
            total_swap_bytes: self.sys.total_swap(),
            used_swap_bytes: self.sys.used_swap(),
            uptime_secs: System::uptime(),
        };

        Snapshot {
            hostname: System::host_name().unwrap_or_else(|| "unknown".to_string()),
            timestamp_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            readings,
            top_processes,
            memory_groups,
            cpu_groups,
            system_stats,
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}
