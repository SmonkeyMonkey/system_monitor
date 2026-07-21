// Installs/uninstalls this agent binary as a per-user macOS LaunchAgent so
// it survives logout/login and gets restarted by launchd if it crashes.
// Uses ~/Library/LaunchAgents (no root/sudo needed), not
// /Library/LaunchDaemons, since this runs in the context of one user.

use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.system-monitor.menubar";

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn plist_path() -> PathBuf {
    home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn log_dir() -> PathBuf {
    home_dir().join("Library/Logs/system_monitor")
}

pub fn install() {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("agent: could not resolve current executable path: {e}");
            std::process::exit(1);
        }
    };

    let log_dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("agent: could not create log dir {}: {e}", log_dir.display());
        std::process::exit(1);
    }

    let launch_agents_dir = plist_path().parent().unwrap().to_path_buf();
    if let Err(e) = std::fs::create_dir_all(&launch_agents_dir) {
        eprintln!(
            "agent: could not create LaunchAgents dir {}: {e}",
            launch_agents_dir.display()
        );
        std::process::exit(1);
    }

    let stdout_log = log_dir.join("menubar.out.log");
    let stderr_log = log_dir.join("menubar.err.log");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{stdout}</string>
    <key>StandardErrorPath</key>
    <string>{stderr}</string>
</dict>
</plist>
"#,
        exe = exe_path.display(),
        stdout = stdout_log.display(),
        stderr = stderr_log.display(),
    );

    let path = plist_path();
    if let Err(e) = std::fs::write(&path, plist) {
        eprintln!("agent: failed to write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("agent: wrote {}", path.display());

    // bootout first in case an older version is already loaded; ignore
    // errors since it's fine if nothing was loaded yet.
    let uid = current_uid();
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .output();

    let status = Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("agent: installed and loaded as a LaunchAgent ({LABEL})");
            println!("agent: logs at {}", log_dir.display());
        }
        Ok(s) => {
            eprintln!("agent: launchctl bootstrap exited with {s}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("agent: failed to run launchctl: {e}");
            std::process::exit(1);
        }
    }
}

pub fn uninstall() {
    let uid = current_uid();
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{LABEL}")])
        .status();

    let path = plist_path();
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!("agent: failed to remove {}: {e}", path.display());
            std::process::exit(1);
        }
        println!("agent: removed {}", path.display());
    }
    println!("agent: uninstalled ({LABEL})");
}

pub fn is_installed() -> bool {
    plist_path().exists()
}

pub fn status() {
    let uid = current_uid();
    let output = Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{LABEL}")])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let state_line = stdout
                .lines()
                .find(|l| l.trim_start().starts_with("state ="))
                .unwrap_or("(state unknown)");
            println!("agent: {LABEL} is loaded, {}", state_line.trim());
        }
        _ => {
            println!("agent: {LABEL} is not loaded");
        }
    }
}

fn current_uid() -> u32 {
    let output = match Command::new("id").arg("-u").output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("agent: failed to run `id -u`: {e}");
            std::process::exit(1);
        }
    };
    match String::from_utf8_lossy(&output.stdout).trim().parse() {
        Ok(uid) => uid,
        Err(e) => {
            eprintln!("agent: unexpected `id -u` output: {e}");
            std::process::exit(1);
        }
    }
}
