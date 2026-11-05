// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 LegendofZito (Kadaken)

//! What a crash says. Without this, a panic in a desktop program started from
//! a launcher prints to a terminal nobody is looking at and the window simply
//! vanishes: the user has nothing to report and nothing to send. The hook here
//! writes one file per crash with the version, the time, the message, where it
//! happened and the backtrace, and names that file on standard error.
//!
//! Deliberately plain: no GTK, no glib, no extra crates, nothing that can
//! itself fail during a panic. Both binaries install it, and `portal_main.rs`
//! can use it without pulling in any of the user interface.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A date a person can read, from a count of seconds, in UTC. Written out by
/// hand because a crash reporter should not depend on a date crate that could
/// itself be the thing that broke. The algorithm is the standard civil-date
/// one: shift the epoch to 1 March so leap days land at the end of the year.
fn utc_stamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let secs_of_day = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60
    )
}

/// Where crash reports go: `$XDG_STATE_HOME/wayfinder`, or
/// `~/.local/state/wayfinder` when that is unset.
pub fn report_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home = std::env::var_os("HOME")?;
            if home.is_empty() {
                return None;
            }
            PathBuf::from(home).join(".local/state")
        }
    };
    Some(base.join("wayfinder"))
}

/// Installs the panic hook. `program` names which binary crashed, so a report
/// from the file dialog is not mistaken for one from the file manager.
pub fn install(program: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let message = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "a panic with no message".to_string()
        };
        let place = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "an unknown place".to_string());

        let mut report = String::new();
        let _ = writeln!(report, "Wayfinder crash report");
        let _ = writeln!(report, "program: {program}");
        let _ = writeln!(report, "version: {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(report, "time: {}", utc_stamp(seconds));
        let _ = writeln!(report, "what happened: {message}");
        let _ = writeln!(report, "where: {place}");
        let _ = writeln!(report, "\nbacktrace:\n{}", std::backtrace::Backtrace::force_capture());

        // Writing the file is best effort. A crash must still be reported on
        // standard error even when the disk is full or the home is read-only.
        let written = report_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join(format!("crash-{program}-{seconds}.log"));
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()?;
            file.write_all(report.as_bytes()).ok()?;
            Some(path)
        });

        eprintln!("\nWayfinder stopped because of a fault it could not recover from.");
        eprintln!("  {message}");
        eprintln!("  in {place} (version {})", env!("CARGO_PKG_VERSION"));
        match written {
            Some(path) => eprintln!(
                "A full report is in {}. Attach it to a bug report at\n\
                 https://github.com/LegendofZito/wayfinder/issues",
                path.display()
            ),
            None => eprintln!(
                "The report could not be saved, so it is printed here instead:\n{report}"
            ),
        }

        // Still do whatever Rust would have done, so nothing that depends on
        // the standard message changes behaviour.
        previous(info);
    }));
}
