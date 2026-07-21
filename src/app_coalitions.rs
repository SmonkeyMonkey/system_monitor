// Maps every PID on the system to the app that macOS considers responsible
// for it, by shelling out to `lsappinfo list` and parsing its output.
//
// Why this exists: sysinfo's Process::parent() reflects the real process
// tree, but macOS runs XPC services (e.g. Safari's per-tab
// com.apple.WebKit.WebContent, or Mail's helper processes) as children of
// launchd (PPID 1), not of the app that requested them — the actual
// ownership link lives in the kernel's process *coalition* mechanism,
// which has no public Rust-facing API. `lsappinfo` (an Apple system tool)
// prints that coalition membership, which is the same data source
// Activity Monitor uses to group these processes under their real app.
//
// This only works for GUI apps registered with the window server
// (launchd-style Foreground/UIElement/BackgroundOnly apps show up here);
// plain CLI processes won't appear and are left for the caller to handle
// via their own fallback (e.g. grouping by executable name).

use std::collections::HashMap;
use std::process::Command;

pub struct AppInfo {
    pub app_name: String,
    pub bundle_path: Option<String>,
}

pub fn pid_to_app_map() -> HashMap<u32, AppInfo> {
    let output = match Command::new("lsappinfo").arg("list").output() {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    parse_lsappinfo(&text)
}

fn parse_lsappinfo(text: &str) -> HashMap<u32, AppInfo> {
    let mut result = HashMap::new();

    // Each app entry looks roughly like:
    //   36) "Safari" ASN:0x0-0x8e08e:
    //       bundleID="com.apple.Safari"
    //       bundle path="/System/.../Safari.app"
    //       ...
    //       pid = 10129 type="Foreground" ...
    //       coalition: 1909  { 10129 10138 ... }
    // Entries are split on lines starting with "N) " at column 0 (no
    // leading whitespace); everything else is indented.
    let mut app_name = String::new();
    let mut bundle_path: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        // Entry headers are right-aligned with leading spaces (e.g.
        // " 4) \"Telegram\" ..." vs "36) \"Safari\" ..."), so the digit
        // check has to happen after trimming, not on the raw line.
        if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
            // Continue past remaining digits and ") ".
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            if let Some(rest) = rest.strip_prefix(") ") {
                if let Some(name) = extract_quoted(rest) {
                    app_name = name;
                    bundle_path = None;
                }
                continue;
            }
        }

        if let Some(rest) = trimmed.strip_prefix("bundle path=") {
            bundle_path = extract_quoted(rest);
        } else if let Some(rest) = trimmed.strip_prefix("coalition:") {
            for pid in extract_pids(rest) {
                result.insert(
                    pid,
                    AppInfo {
                        app_name: app_name.clone(),
                        bundle_path: bundle_path.clone(),
                    },
                );
            }
        }
    }

    result
}

fn extract_quoted(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let end = start + s[start..].find('"')?;
    Some(s[start..end].to_string())
}

fn extract_pids(coalition_line: &str) -> Vec<u32> {
    // "coalition:" is followed by the coalition id, then optionally a
    // brace-delimited member list: "1909  { 10129 10138 ... }". If there's
    // no brace list, the id itself isn't a PID we care about — only the
    // member PIDs (which include the main app's own pid) matter here.
    let Some(open) = coalition_line.find('{') else {
        return Vec::new();
    };
    let Some(close) = coalition_line[open..].find('}') else {
        return Vec::new();
    };
    coalition_line[open + 1..open + close]
        .split_whitespace()
        .filter_map(|tok| tok.parse().ok())
        .collect()
}
