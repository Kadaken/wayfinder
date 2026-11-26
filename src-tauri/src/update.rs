// Telling people a new Wayfinder exists, and installing it for them.
//
// Wayfinder is handed out as a tarball people unpack into their home
// directory. Nothing on the machine owns it: `dnf` has never heard of it, so
// it cannot appear in a system update, and until this module existed there was
// no mechanism of any kind for saying a newer version was out. The download on
// kadaken.com sat at 0.9.11 while 0.9.23 was current, and every single person
// who had installed it was twelve releases behind with no way to find out.
//
// Three things are deliberately NOT done here.
//
// No new crate. The check is one small HTTPS GET, and `curl` is in Fedora's
// base install, so shelling out to it costs nothing and leaves no TLS stack
// vendored into this binary to keep patched. `--proto =https --tlsv1.2` and
// an explicit `--max-time` are passed every time rather than trusted to a
// default.
//
// No install over a running binary. `packaging/install.sh` uses
// `install -m 0755`, which opens the destination with O_TRUNC, and Linux
// refuses that on the image of a running process with ETXTBSY. So an update
// downloads and verifies while Wayfinder runs, and the install happens from a
// detached helper AFTER this process has gone. Doing it the obvious way would
// have failed on the one file that matters, every time.
//
// No silent download. The check is quiet, the download is not: an update is
// several megabytes over somebody's connection and it replaces the file
// manager they are in the middle of using, so it waits to be asked.
//
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Kadaken

use serde::Deserialize;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Where the manifest lives. Served by kadaken.com beside the download it
/// describes, and written by `scripts/make-update-manifest.sh` off the same
/// tarball, so the two cannot disagree about a version or a checksum.
pub const MANIFEST_URL: &str = "https://kadaken.com/wayfinder-update.json";

/// How long a check is allowed to take before it is abandoned. A file manager
/// must never wait on a web server, so this is short and the whole check runs
/// off the main thread anyway.
const CHECK_TIMEOUT_SECS: &str = "10";

/// Seconds between checks. Once a day: often enough that a release is found
/// within a day of shipping, rare enough that opening ten windows does not
/// mean ten requests.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// What the server says the newest release is.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// Three dot-separated numbers, as in Cargo.toml.
    pub version: String,
    /// Where the tarball is. Checked against the expected host before use:
    /// a manifest is fetched over the network, so it is input, not truth.
    pub url: String,
    /// Lowercase hex sha256 of the tarball at `url`.
    pub sha256: String,
    /// Bytes, so a truncated download is caught before it is unpacked.
    pub size: u64,
    /// One line for the dialog. Optional: a release with nothing worth
    /// saying should not be blocked from shipping.
    #[serde(default)]
    pub notes: String,
}

/// The only host an update may be downloaded from. A manifest arrives over the
/// network and could name any address at all; without this, poisoning the
/// manifest would be enough to have Wayfinder fetch and unpack a stranger's
/// archive. Pinned to the site rather than merely to https.
const ALLOWED_HOST_PREFIXES: [&str; 2] = ["https://kadaken.com/", "https://www.kadaken.com/"];

/// True when `url` points at the real download host.
pub fn url_is_allowed(url: &str) -> bool {
    ALLOWED_HOST_PREFIXES
        .iter()
        .any(|prefix| url.starts_with(prefix))
}

/// A version as numbers, so it can be compared as numbers.
///
/// String comparison is the trap this exists to avoid: `"0.9.9" > "0.9.23"`
/// is true for text and false for software, and getting it wrong means an
/// update stops being offered the moment a patch number reaches double
/// digits -- which 0.9.23 already has.
fn parts(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| {
            // Take the leading digits and ignore any suffix, so a tag like
            // "0.9.24-beta1" still compares as 0.9.24 rather than as nothing.
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

/// True when `candidate` is a strictly newer release than `current`.
///
/// Missing trailing parts count as zero, so "0.10" is newer than "0.9.23"
/// and the same as "0.10.0".
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parts(candidate), parts(current));
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// A manifest is only usable if every field it will be acted on is present
/// and sane. Rejecting here means the rest of the module never has to wonder.
pub fn validate(manifest: &Manifest) -> Result<(), String> {
    if parts(&manifest.version).iter().all(|n| *n == 0) {
        return Err(format!("version {:?} is not a version", manifest.version));
    }
    if !url_is_allowed(&manifest.url) {
        return Err(format!("{} is not on the download host", manifest.url));
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("sha256 is not a 64-character hex digest".to_string());
    }
    // A Wayfinder tarball is a few megabytes. These bounds only catch a
    // manifest that is obviously wrong, such as a zero size or a figure that
    // would fill the disk.
    if manifest.size < 100_000 || manifest.size > 200_000_000 {
        return Err(format!("{} bytes is not a plausible download", manifest.size));
    }
    Ok(())
}

fn manifest_curl_args() -> &'static [&'static str] {
    &[
        "--disable",
        "--silent",
        "--show-error",
        "--fail",
        // D-427: the manifest is served directly. Refuse redirects.
        "--location",
        "--max-redirs",
        "0",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--max-time",
        CHECK_TIMEOUT_SECS,
        // A version number is all the server needs and all it is told.
        "--user-agent",
        concat!("Wayfinder/", env!("CARGO_PKG_VERSION")),
        // D-278: a cached manifest can hide a newly published release.
        "--header",
        "Cache-Control: no-cache",
        MANIFEST_URL,
    ]
}

/// Fetches and parses the manifest. Blocking: callers run it off the main
/// thread. Every failure is a plain `Err` -- a check that cannot reach the
/// network is a normal Tuesday, not a fault to report to anybody.
pub fn fetch_manifest() -> Result<Manifest, String> {
    let output = Command::new("curl")
        .args(manifest_curl_args())
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "fetching the manifest failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("the manifest is not the shape it should be: {e}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

/// The version this binary is.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Where a download is staged. Its own directory under the cache, wiped and
/// recreated per attempt so a half-finished earlier try cannot be mistaken
/// for this one's work.
fn staging_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("wayfinder").join("update"))
}

/// Lowercase hex sha256 of a file on disk, read in chunks so a large archive
/// is never held in memory all at once.
fn sha256_of(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn download_curl_args<'a>(archive: &'a Path, url: &'a str) -> impl Iterator<Item = &'a OsStr> {
    [
        "--disable",
        "--globoff",
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--max-time",
        "600",
        "--user-agent",
        concat!("Wayfinder/", env!("CARGO_PKG_VERSION")),
        "--write-out",
        "%{url_effective}",
        "--output",
    ]
    .into_iter()
    .map(OsStr::new)
    .chain([archive.as_os_str(), OsStr::new(url)])
}

fn download_archive(
    archive: &Path,
    url: &str,
    run: impl FnOnce(&mut Command) -> std::io::Result<Output>,
) -> Result<(), String> {
    let output = run(Command::new("curl").args(download_curl_args(archive, url)))
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !output.status.success() {
        return Err("the download did not finish".to_string());
    }
    // D-427: /get/ redirects to the archive. Use it only if the final URL
    // is on an allowed HTTPS host.
    if !std::str::from_utf8(&output.stdout).is_ok_and(url_is_allowed) {
        return Err("the update download ended on an unapproved URL".to_string());
    }
    Ok(())
}

/// Downloads the release named by `manifest`, checks its size and checksum,
/// and unpacks it. Returns the unpacked directory -- the one holding
/// `packaging/install.sh`.
///
/// Nothing is unpacked before the checksum matches. Unpacking first and
/// verifying after would mean writing a stranger's archive to disk and only
/// then deciding whether to trust it.
pub fn download_and_verify(manifest: &Manifest) -> Result<PathBuf, String> {
    validate(manifest)?;
    let dir = staging_dir().ok_or("no cache directory to download into")?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let archive = dir.join("wayfinder-update.tar.gz");

    download_archive(&archive, &manifest.url, Command::output)?;

    let got = std::fs::metadata(&archive)
        .map_err(|e| format!("{}: {e}", archive.display()))?
        .len();
    if got != manifest.size {
        return Err(format!(
            "the download is {got} bytes, not the {} the manifest says",
            manifest.size
        ));
    }
    let digest = sha256_of(&archive)?;
    if !digest.eq_ignore_ascii_case(&manifest.sha256) {
        return Err("the download does not match its checksum and was not used".to_string());
    }

    let unpacked = dir.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| format!("{}: {e}", unpacked.display()))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .map_err(|e| format!("could not run tar: {e}"))?;
    if !status.success() {
        return Err("the archive would not unpack".to_string());
    }
    // The tarball holds one top-level directory, named for its version.
    let root = std::fs::read_dir(&unpacked)
        .map_err(|e| format!("{}: {e}", unpacked.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.join("packaging").join("install.sh").is_file())
        .ok_or("the archive has no packaging/install.sh in it")?;
    Ok(root)
}

/// The helper that does the install once Wayfinder is gone.
///
/// It waits for this process to exit rather than sleeping a guessed number of
/// seconds, because the whole reason it exists is ETXTBSY: `install` cannot
/// write the binary while that binary is still running, and a fixed sleep
/// would turn that into a race that fails sometimes instead of a wait that
/// works always. The PID is passed in and polled with `kill -0`.
fn helper_script(pid: u32, root: &Path, relaunch: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
# Written by Wayfinder to finish an update. Safe to delete.
set -uo pipefail
for _ in $(seq 1 600); do
  kill -0 {pid} 2>/dev/null || break
  sleep 0.5
done
# One last check: if Wayfinder is somehow still running, stop rather than
# fail halfway through replacing its files.
if kill -0 {pid} 2>/dev/null; then
  exit 1
fi
cd {root}
./packaging/install.sh >"$HOME/.cache/wayfinder/update/install.log" 2>&1 || exit 1
setsid {relaunch} >/dev/null 2>&1 &
"#,
        pid = pid,
        root = shell_quote(&root.display().to_string()),
        relaunch = shell_quote(&relaunch.display().to_string()),
    )
}

/// Single-quotes a path for the shell. Paths come from the user's own home
/// directory and may hold spaces or quotes.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'\''"#))
}

/// Writes the helper and starts it detached, so it outlives this process.
/// Returns once the helper is running; the caller then quits the app.
pub fn install_after_exit(root: &Path) -> Result<(), String> {
    let dir = staging_dir().ok_or("no cache directory")?;
    let script = dir.join("finish-update.sh");
    // Relaunch whatever was actually run, so an install under a non-default
    // prefix comes back from its own prefix rather than a guessed one.
    let relaunch = std::env::current_exe().map_err(|e| format!("cannot find my own path: {e}"))?;
    std::fs::write(&script, helper_script(std::process::id(), root, &relaunch))
        .map_err(|e| format!("{}: {e}", script.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", script.display()))?;
    }
    Command::new("setsid")
        .arg(&script)
        .spawn()
        .map_err(|e| format!("could not start the installer: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_digit_patches_compare_as_numbers_not_text() {
        // The whole reason `parts` exists. As text, "0.9.9" sorts after
        // "0.9.23" and the update would never be offered again.
        assert!(is_newer("0.9.23", "0.9.9"));
        assert!(!is_newer("0.9.9", "0.9.23"));
    }

    #[test]
    fn same_version_is_not_an_update() {
        assert!(!is_newer("0.9.23", "0.9.23"));
        assert!(!is_newer("v0.9.23", "0.9.23"));
    }

    #[test]
    fn missing_parts_count_as_zero() {
        assert!(is_newer("0.10", "0.9.23"));
        assert!(!is_newer("0.10", "0.10.0"));
        assert!(is_newer("1", "0.99.99"));
    }

    #[test]
    fn a_suffix_does_not_erase_the_number() {
        assert!(is_newer("0.9.24-beta1", "0.9.23"));
        assert!(!is_newer("0.9.23-beta1", "0.9.23"));
    }

    #[test]
    fn only_the_download_host_is_accepted() {
        assert!(url_is_allowed("https://kadaken.com/get/wayfinder"));
        assert!(url_is_allowed("https://kadaken.com/download/wayfinder-linux-x64.tar.gz"));
        assert!(url_is_allowed("https://www.kadaken.com/download/x.tar.gz"));
        assert!(!url_is_allowed("http://kadaken.com/get/wayfinder"));
        assert!(!url_is_allowed("https://kadaken.com.evil.net/x"));
        assert!(!url_is_allowed("https://evil.test/get/wayfinder"));
        assert!(!url_is_allowed(""));
    }

    #[test]
    fn curl_arguments_enforce_https_and_the_manifest_redirect_limit_d448() {
        let manifest_args = manifest_curl_args();
        assert_eq!(manifest_args.first(), Some(&"--disable"));
        assert!(manifest_args.contains(&"--location"));
        assert!(manifest_args.windows(2).any(|args| args == ["--max-redirs", "0"]));
        assert!(manifest_args.windows(2).any(|args| args == ["--proto", "=https"]));

        let manifest = good();
        let download: Vec<_> = download_curl_args(Path::new("update.tar.gz"), &manifest.url)
            .map(|arg| arg.to_str().unwrap())
            .collect();
        assert_eq!(download[0], "--disable");
        assert!(download.contains(&"--location"));
        assert!(download.windows(2).any(|args| args == ["--proto", "=https"]));
        assert!(download.windows(2).any(|args| args == ["--proto-redir", "=https"]));
        assert!(download.windows(2).any(|args| args == ["--write-out", "%{url_effective}"]));
    }

    #[test]
    fn a_successful_curl_with_an_unapproved_effective_url_is_refused_d448() {
        use std::os::unix::process::ExitStatusExt;

        let archive = Path::new("update.tar.gz");
        let url = good().url;
        let curl_output = |effective_url: &str| Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: effective_url.as_bytes().to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            download_archive(archive, &url, |_| {
                Ok(curl_output("https://kadaken.com/download/wayfinder.tar.gz"))
            }),
            Ok(())
        );
        let error = download_archive(archive, &url, |_| {
            Ok(curl_output("https://example.org/download/wayfinder.tar.gz"))
        })
        .unwrap_err();
        assert_eq!(error, "the update download ended on an unapproved URL");
    }

    fn good() -> Manifest {
        Manifest {
            version: "0.9.24".into(),
            url: "https://kadaken.com/get/wayfinder".into(),
            sha256: "b".repeat(64),
            size: 3_662_829,
            notes: String::new(),
        }
    }

    #[test]
    fn a_good_manifest_validates() {
        assert_eq!(validate(&good()), Ok(()));
    }

    #[test]
    fn a_manifest_naming_another_host_is_refused() {
        let mut m = good();
        m.url = "https://evil.test/wayfinder.tar.gz".into();
        assert!(validate(&m).is_err());
    }

    #[test]
    fn a_bad_digest_is_refused() {
        let mut m = good();
        m.sha256 = "not a digest".into();
        assert!(validate(&m).is_err());
        m.sha256 = "b".repeat(63);
        assert!(validate(&m).is_err());
    }

    #[test]
    fn an_implausible_size_is_refused() {
        let mut m = good();
        m.size = 0;
        assert!(validate(&m).is_err());
        m.size = 500_000_000;
        assert!(validate(&m).is_err());
    }

    #[test]
    fn a_nonsense_version_is_refused() {
        let mut m = good();
        m.version = "latest".into();
        assert!(validate(&m).is_err());
    }

    #[test]
    fn notes_are_optional() {
        let parsed: Manifest = serde_json::from_str(
            r#"{"version":"0.9.24","url":"https://kadaken.com/get/wayfinder",
                "sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "size":3662829}"#,
        )
        .expect("a manifest without notes still parses");
        assert_eq!(parsed.notes, "");
    }

    #[test]
    fn the_helper_waits_for_the_process_rather_than_sleeping_a_guess() {
        let script = helper_script(4242, Path::new("/tmp/x"), Path::new("/usr/bin/wayfinder"));
        assert!(script.contains("kill -0 4242"), "it must poll the real pid");
        assert!(
            script.contains("./packaging/install.sh"),
            "it must run the installer the tarball ships"
        );
        assert!(
            script.contains("exit 1"),
            "it must give up rather than install over a running binary"
        );
    }

    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_out_of_the_helper() {
        let script = helper_script(1, Path::new("/tmp/it's here; rm -rf ~"), Path::new("/bin/x"));
        assert!(script.contains(r#"'/tmp/it'\''s here; rm -rf ~'"#));
        assert!(!script.contains("; rm -rf ~\n"));
    }
}
