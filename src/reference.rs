// Static "normal" ranges as a fallback for components with no history yet.
// Values are rough consumer-hardware norms (idle-to-load range) in Celsius.

pub struct ReferenceRange {
    pub normal_low: f32,
    pub normal_high: f32,
}

pub fn lookup(label: &str) -> ReferenceRange {
    match category(label) {
        Category::Cpu => ReferenceRange {
            normal_low: 35.0,
            normal_high: 75.0,
        },
        Category::Gpu => ReferenceRange {
            normal_low: 35.0,
            normal_high: 80.0,
        },
        Category::Storage => ReferenceRange {
            normal_low: 25.0,
            normal_high: 55.0,
        },
        Category::Battery => ReferenceRange {
            normal_low: 20.0,
            normal_high: 45.0,
        },
        Category::Other => ReferenceRange {
            normal_low: 30.0,
            normal_high: 70.0,
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Category {
    Cpu,
    Gpu,
    Storage,
    Battery,
    Other,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::Cpu => "CPU",
            Category::Gpu => "GPU",
            Category::Storage => "Storage",
            Category::Battery => "Battery",
            Category::Other => "Other",
        }
    }
}

// Maps raw sensor labels (often vendor-internal SMC names like "PMU tdie6"
// or "NAND CH0 temp") to a human-facing category. Apple doesn't publish an
// official mapping from these SMC keys to specific components, so this is
// a best-effort heuristic based on common naming patterns.
pub fn category(label: &str) -> Category {
    let l = label.to_lowercase();
    if l.contains("gpu") {
        Category::Gpu
    } else if l.contains("cpu")
        || l.contains("core")
        || l.contains("package")
        || l.contains("tctl")
        || l.contains("tdie")
        || l.contains("pmu")
    {
        Category::Cpu
    } else if l.contains("ssd") || l.contains("nvme") || l.contains("disk") || l.contains("nand") {
        Category::Storage
    } else if l.contains("battery") || l.contains("gas gauge") {
        Category::Battery
    } else {
        Category::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_known_sensor_labels() {
        assert_eq!(category("GPU die temp"), Category::Gpu);
        assert_eq!(category("PMU tdie6"), Category::Cpu);
        assert_eq!(category("CPU Core 3"), Category::Cpu);
        assert_eq!(category("Tctl"), Category::Cpu);
        assert_eq!(category("SSD NAND CH0 temp"), Category::Storage);
        assert_eq!(category("NVMe temp"), Category::Storage);
        assert_eq!(category("gas gauge battery"), Category::Battery);
        assert_eq!(category("Battery Cell"), Category::Battery);
        assert_eq!(category("some unknown sensor"), Category::Other);
    }

    #[test]
    fn category_matching_is_case_insensitive() {
        assert_eq!(category("gpu"), Category::Gpu);
        assert_eq!(category("GPU"), Category::Gpu);
        assert_eq!(category("Gpu Die"), Category::Gpu);
    }

    #[test]
    fn gpu_takes_priority_over_cpu_keywords_when_both_present() {
        // A label containing both "gpu" and a CPU-like keyword should still
        // be classified as GPU, since that check runs first.
        assert_eq!(category("GPU Core"), Category::Gpu);
    }

    #[test]
    fn lookup_returns_matching_range_per_category() {
        let cpu = lookup("PMU tdie6");
        assert_eq!(cpu.normal_low, 35.0);
        assert_eq!(cpu.normal_high, 75.0);

        let battery = lookup("gas gauge battery");
        assert_eq!(battery.normal_low, 20.0);
        assert_eq!(battery.normal_high, 45.0);

        let other = lookup("mystery sensor");
        assert_eq!(other.normal_low, 30.0);
        assert_eq!(other.normal_high, 70.0);
    }
}
