use system_monitor::Collector;

fn main() {
    let mut collector = Collector::new();
    // A second collection tick is needed so sysinfo reports real CPU deltas
    // instead of 0% on the first sample.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let snapshot = collector.collect();

    println!("System Monitor\n==============\n");

    if snapshot.readings.is_empty() {
        println!("No temperature sensors were readable on this system.");
        println!(
            "(macOS often requires elevated privileges or a helper like `sudo powermetrics` for SMC sensors.)"
        );
        return;
    }

    for reading in &snapshot.readings {
        print!("{}: {:.1}\u{00B0}C", reading.label, reading.temperature);
        match (reading.baseline_mean, reading.baseline_stddev) {
            (Some(mean), Some(stddev)) => {
                let delta = reading.temperature - mean;
                println!(
                    " | your baseline: {:.1}\u{00B0}C \u{00B1} {:.1} ({} samples) | {}{:.1}\u{00B0}C vs your normal",
                    mean,
                    stddev,
                    reading.baseline_samples,
                    if delta >= 0.0 { "+" } else { "" },
                    delta
                );
            }
            _ => {
                println!(
                    " | building your baseline ({}/5 samples so far) | reference normal: {:.0}-{:.0}\u{00B0}C",
                    reading.baseline_samples, reading.reference_low, reading.reference_high
                );
            }
        }

        if reading.is_hot {
            if reading.suspects.is_empty() {
                println!("  No single process stands out; check ambient temp / cooling / dust.");
            } else {
                println!("  Likely contributors (top CPU usage right now):");
                for s in &reading.suspects {
                    println!("    - {} ({:.1}% CPU)", s.name, s.cpu_usage);
                }
            }
        }
        println!();
    }
}
