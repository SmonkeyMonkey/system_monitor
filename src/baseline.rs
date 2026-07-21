use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const MAX_SAMPLES_PER_COMPONENT: usize = 500;

#[derive(Serialize, Deserialize, Default)]
pub struct History {
    pub samples: HashMap<String, Vec<f32>>,
}

impl History {
    pub fn path() -> PathBuf {
        let dir = dirs_home().join(".system_monitor");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("history.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), json);
        }
    }

    pub fn record(&mut self, label: &str, temp: f32) {
        let entries = self.samples.entry(label.to_string()).or_default();
        entries.push(temp);
        if entries.len() > MAX_SAMPLES_PER_COMPONENT {
            let excess = entries.len() - MAX_SAMPLES_PER_COMPONENT;
            entries.drain(0..excess);
        }
    }

    pub fn baseline_for(&self, label: &str) -> Option<Baseline> {
        let entries = self.samples.get(label)?;
        if entries.len() < 5 {
            return None;
        }
        let mean = entries.iter().sum::<f32>() / entries.len() as f32;
        let variance =
            entries.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / entries.len() as f32;
        let stddev = variance.sqrt();
        // entries always has at least 5 elements at this point, so max() is Some.
        let recent_max = entries.iter().copied().fold(f32::MIN, f32::max);
        Some(Baseline {
            mean,
            stddev,
            sample_count: entries.len(),
            recent_max,
        })
    }
}

pub struct Baseline {
    pub mean: f32,
    pub stddev: f32,
    pub sample_count: usize,
    // Highest temperature seen across the retained sample window (up to
    // MAX_SAMPLES_PER_COMPONENT most recent samples, not all-time).
    pub recent_max: f32,
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_for_returns_none_below_minimum_sample_count() {
        let mut history = History::default();
        for _ in 0..4 {
            history.record("cpu0", 50.0);
        }
        assert!(history.baseline_for("cpu0").is_none());
    }

    #[test]
    fn baseline_for_returns_some_at_minimum_sample_count() {
        let mut history = History::default();
        for _ in 0..5 {
            history.record("cpu0", 50.0);
        }
        assert!(history.baseline_for("cpu0").is_some());
    }

    #[test]
    fn baseline_for_computes_mean_stddev_and_recent_max() {
        let mut history = History::default();
        for temp in [10.0, 20.0, 30.0, 40.0, 50.0] {
            history.record("cpu0", temp);
        }
        let baseline = history.baseline_for("cpu0").unwrap();
        assert_eq!(baseline.sample_count, 5);
        assert_eq!(baseline.mean, 30.0);
        assert_eq!(baseline.recent_max, 50.0);
        // variance = mean((x - 30)^2) for [10,20,30,40,50] = 200, stddev = sqrt(200)
        assert!((baseline.stddev - 200f32.sqrt()).abs() < 1e-3);
    }

    #[test]
    fn baseline_for_unknown_label_is_none() {
        let history = History::default();
        assert!(history.baseline_for("nonexistent").is_none());
    }

    #[test]
    fn record_caps_samples_per_component_and_drops_oldest() {
        let mut history = History::default();
        for i in 0..(MAX_SAMPLES_PER_COMPONENT + 10) {
            history.record("cpu0", i as f32);
        }
        let entries = &history.samples["cpu0"];
        assert_eq!(entries.len(), MAX_SAMPLES_PER_COMPONENT);
        // The oldest 10 samples (0..10) should have been dropped, so the
        // first remaining sample is 10.0, and the last is the most recent.
        assert_eq!(entries[0], 10.0);
        assert_eq!(
            *entries.last().unwrap(),
            (MAX_SAMPLES_PER_COMPONENT + 9) as f32
        );
    }

    #[test]
    fn record_tracks_components_independently() {
        let mut history = History::default();
        history.record("cpu0", 10.0);
        history.record("gpu0", 90.0);
        assert_eq!(history.samples["cpu0"], vec![10.0]);
        assert_eq!(history.samples["gpu0"], vec![90.0]);
    }
}
