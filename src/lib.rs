//! The shared installer/updater engine of the Jay apps: finds a GitHub repo's latest release,
//! picks its Windows installer, downloads it and runs it only if its SHA-256 matches the `digest`
//! GitHub computed. Also reads the installed version from the per-user uninstall registry key.

mod http;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// No Jay installer comes near this (Jay-C 0.4.0: 2.3 MB; Jay-A plans up to 20 MB).
pub const MAX_INSTALLER: u64 = 200 * 1024 * 1024;
/// The release JSON (a few KB of notes and assets).
pub const MAX_JSON: u64 = 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
    pub digest: Option<String>,
}

/// A release's installer after every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// "owner/name": one slash, plain characters, no dot-only or `..` parts.
fn valid_repo(repo: &str) -> bool {
    let mut halves = repo.split('/');
    let (Some(owner), Some(name), None) = (halves.next(), halves.next(), halves.next()) else {
        return false;
    };
    [owner, name].iter().all(|h| {
        !h.is_empty() && *h != "." && !h.contains("..") && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    })
}

/// The product name becomes a registry path, so only plain characters.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The latest release of `repo` ("owner/name"), or None when it has none yet.
pub fn latest(repo: &str, user_agent: &str) -> Result<Option<Release>, String> {
    if !valid_repo(repo) {
        return Err(format!("invalid repository: {repo}"));
    }
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let response = http::get(&url, user_agent)?;
    match response.status {
        404 => Ok(None),
        200 => serde_json::from_reader(response.take(MAX_JSON)).map(Some).map_err(|e| e.to_string()),
        status => Err(format!("GitHub answered {status}")),
    }
}

fn plain_name(name: &str) -> bool {
    !name.contains(['/', '\\', ':'])
}

/// The release's x64 installer and its SHA-256: from this repository's release downloads, a plain
/// file name, a sane size, and a digest from GitHub.
pub fn pick_installer(repo: &str, release: &Release) -> Result<Installer, String> {
    if !valid_repo(repo) {
        return Err(format!("invalid repository: {repo}"));
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with("_x64-setup.exe"))
        .ok_or("the latest release has no Windows installer")?;
    if !asset.browser_download_url.starts_with(&format!("https://github.com/{repo}/releases/download/")) {
        return Err(format!("unexpected download address: {}", asset.browser_download_url));
    }
    if !plain_name(&asset.name) || asset.size > MAX_INSTALLER {
        return Err(format!("unexpected installer: {} ({} bytes)", asset.name, asset.size));
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("the installer has no SHA-256 on GitHub, so it can't be checked")?
        .to_ascii_lowercase();
    Ok(Installer { name: asset.name.clone(), url: asset.browser_download_url.clone(), size: asset.size, sha256 })
}

/// Copies `from` into `to` and returns its SHA-256 (lowercase hex) and length. Fails past `limit` bytes.
fn copy_hashed(mut from: impl Read, mut to: impl Write, limit: u64, mut progress: impl FnMut(u64)) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(io::Error::other("the download is bigger than GitHub said"));
        }
        hasher.update(&buf[..n]);
        to.write_all(&buf[..n])?;
        progress(total);
    }
    to.flush()?;
    Ok((hasher.finalize().iter().map(|b| format!("{b:02x}")).collect(), total))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary folder name no other call uses (pid, time and a counter).
fn fresh_dir() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("jay-i-{}-{nanos}-{n}", std::process::id()))
}

/// Creates `dir` (it must not exist) and saves `body` in it as the installer, checking length and
/// SHA-256. On any failure the folder is removed.
fn store(dir: &Path, body: impl Read, installer: &Installer, mut on_progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    fs::create_dir(dir).map_err(|e| e.to_string())?;
    let path = dir.join(&installer.name);
    let result = (|| {
        let file = fs::File::create(&path).map_err(|e| e.to_string())?;
        let (got, len) = copy_hashed(body, file, installer.size, |done| on_progress(done, installer.size)).map_err(|e| e.to_string())?;
        if len != installer.size || got != installer.sha256 {
            return Err(format!("{} doesn't match GitHub's SHA-256; it was not run", installer.name));
        }
        Ok(path)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(dir);
    }
    result
}

/// Downloads the installer into a new temporary folder and checks its SHA-256. On any failure
/// nothing is left behind; on success the folder stays (the caller runs from it, then may remove
/// `path.parent()`).
pub fn download(installer: &Installer, user_agent: &str, on_progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    // The fields are public: a caller could have built this by hand.
    if !installer.name.ends_with("_x64-setup.exe") || !plain_name(&installer.name) || installer.size > MAX_INSTALLER {
        return Err(format!("unexpected installer: {} ({} bytes)", installer.name, installer.size));
    }
    let response = http::get(&installer.url, user_agent)?;
    if response.status != 200 {
        return Err(format!("{}: HTTP {}", installer.url, response.status));
    }
    // `store` stops past the size GitHub gave.
    store(&fresh_dir(), response, installer, on_progress)
}

pub enum Mode {
    Interactive,
    Update { relaunch: bool },
}

/// The command that runs the installer (not spawned). Interactive: the installer's own window.
/// Update: `/P` shows only the progress bar, `/UPDATE` keeps the settings, `/R` (when `relaunch`)
/// opens the app again at the end. A self-updating app spawns it and exits; a hub waits with
/// `.status()`.
pub fn run(installer: &Path, mode: Mode) -> Command {
    let mut command = Command::new(installer);
    match mode {
        Mode::Interactive => {}
        Mode::Update { relaunch: false } => {
            command.args(["/P", "/UPDATE"]);
        }
        Mode::Update { relaunch: true } => {
            command.args(["/P", "/UPDATE", "/R"]);
        }
    }
    command
}

/// An app installed by its NSIS installer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub location: String,
}

/// Version and folder of the installed product, or None.
// shortcut: only per-user installs (HKCU), which is how the Jay installers install; add HKLM if one
// ever installs per machine.
pub fn installed(name: &str) -> Result<Option<Installed>, String> {
    if !valid_name(name) {
        return Err(format!("invalid product name: {name}"));
    }
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        let path = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{name}");
        let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(path) else {
            return Ok(None);
        };
        let version: String = key.get_value("DisplayVersion").map_err(|e| e.to_string())?;
        let location: String = key.get_value("InstallLocation").unwrap_or_default();
        // NSIS stores the folder in quotes.
        Ok(Some(Installed { version, location: location.trim_matches('"').to_owned() }))
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}

/// "v1.2.3-beta+x" -> [1, 2, 3]; None unless every part is plain digits.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let v = v.strip_prefix(['v', 'V']).unwrap_or(v);
    let core = v.split(['-', '+']).next()?;
    core.split('.')
        .map(|p| if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None })
        .collect()
}

/// True only when `latest` is a higher version than `current`; false when either doesn't parse.
pub fn newer(latest: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse_version(latest), parse_version(current)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that create or count `jay-i-*` folders in the temp dir run one at a time.
    static DIRS: Mutex<()> = Mutex::new(());

    const REPO: &str = "TyxMatty/Jay-C";
    const URL: &str = "https://github.com/TyxMatty/Jay-C/releases/download/v0.4.0/Jay-C_0.4.0_x64-setup.exe";
    const DIGEST: &str = "sha256:848005e8a2f42200b36f5553fd144a9d8aefdcbddbbf1ad49b56c65f473d63de";
    const E_URL: &str = "https://github.com/TyxMatty/Jay-E/releases/download/v0.2.0/Jay-E_0.2.0_x64-setup.exe";

    fn release_with(name: &str, size: u64, url: &str, digest: Option<&str>) -> Release {
        Release {
            tag_name: "v0.4.0".into(),
            html_url: "https://github.com/TyxMatty/Jay-C/releases/tag/v0.4.0".into(),
            assets: vec![ReleaseAsset { name: name.into(), size, browser_download_url: url.into(), digest: digest.map(Into::into) }],
        }
    }

    fn release(url: &str, digest: Option<&str>) -> Release {
        release_with("Jay-C_0.4.0_x64-setup.exe", 2302415, url, digest)
    }

    #[test]
    fn pick_installer_checks_address_and_digest() {
        let installer = pick_installer(REPO, &release(URL, Some(DIGEST))).unwrap();
        assert_eq!(installer.name, "Jay-C_0.4.0_x64-setup.exe");
        assert_eq!((installer.url.as_str(), installer.size, installer.sha256.as_str()), (URL, 2302415, &DIGEST[7..]));
        // Another repository's file, another host, no digest, a malformed digest, no installer.
        assert!(pick_installer("TyxMatty/Jay-A", &release(URL, Some(DIGEST))).is_err());
        assert!(pick_installer(REPO, &release("https://example.com/TyxMatty/Jay-C/releases/download/x.exe", Some(DIGEST))).is_err());
        assert!(pick_installer(REPO, &release(URL, None)).is_err());
        assert!(pick_installer(REPO, &release(URL, Some("sha256:abc"))).is_err());
        assert!(pick_installer(REPO, &Release { assets: vec![], ..release(URL, None) }).is_err());
        // Jay-E's constants.
        let e = release_with("Jay-E_0.2.0_x64-setup.exe", 5_000_000, E_URL, Some(DIGEST));
        assert_eq!(pick_installer("TyxMatty/Jay-E", &e).unwrap().name, "Jay-E_0.2.0_x64-setup.exe");
        assert!(pick_installer("TyxMatty/Jay-C", &e).is_err());
        assert!(pick_installer("TyxMatty/Jay-E", &release_with("Jay-E_0.2.0_x64-setup.exe", 5, &E_URL.replace("github.com", "example.com"), Some(DIGEST))).is_err());
    }

    #[test]
    fn pick_installer_rejects_bad_addresses() {
        for url in [
            "https://github.com.evil.example/TyxMatty/Jay-C/releases/download/v0.4.0/x_x64-setup.exe",
            "https://github.com@evil.example/TyxMatty/Jay-C/releases/download/v0.4.0/x_x64-setup.exe",
            "https://github.com/TyxMatty/Jay-A/releases/download/v0.4.0/x_x64-setup.exe",
            "http://github.com/TyxMatty/Jay-C/releases/download/v0.4.0/x_x64-setup.exe",
            "https://github.com/TyxMatty/Jay-C-evil/releases/download/v0.4.0/x_x64-setup.exe",
        ] {
            assert!(pick_installer(REPO, &release(url, Some(DIGEST))).is_err(), "{url}");
        }
    }

    #[test]
    fn pick_installer_rejects_bad_digests() {
        let hex64 = &DIGEST[7..];
        for digest in [
            None,
            Some("sha256:abc".to_owned()),
            Some(format!("sha256:{}g", &hex64[1..])),
            Some(format!("md5:{hex64}")),
            Some(format!("sha256:{hex64}0")),
        ] {
            assert!(pick_installer(REPO, &release(URL, digest.as_deref())).is_err(), "{digest:?}");
        }
        let upper = format!("sha256:{}", hex64.to_ascii_uppercase());
        assert_eq!(pick_installer(REPO, &release(URL, Some(&upper))).unwrap().sha256, hex64);
    }

    #[test]
    fn pick_installer_checks_size_and_name() {
        assert!(pick_installer(REPO, &release_with("a_x64-setup.exe", MAX_INSTALLER, URL, Some(DIGEST))).is_ok());
        assert!(pick_installer(REPO, &release_with("a_x64-setup.exe", MAX_INSTALLER + 1, URL, Some(DIGEST))).is_err());
        for name in ["..\\x_x64-setup.exe", "C:x_x64-setup.exe", "../x_x64-setup.exe"] {
            assert!(pick_installer(REPO, &release_with(name, 10, URL, Some(DIGEST))).is_err(), "{name}");
        }
    }

    #[test]
    fn invalid_repos_are_rejected() {
        let r = release(URL, Some(DIGEST));
        for repo in ["TyxMatty/../x", "TyxMatty/..", "TyxMatty\\Jay-C", "TyxMatty/Jay-C/x", "/Jay-C", "", "TyxMatty", "./x", "a b/c"] {
            assert!(pick_installer(repo, &r).is_err(), "{repo:?}");
            assert!(latest(repo, "Jay-I test").is_err(), "{repo:?}");
        }
    }

    #[test]
    fn copy_hashed_hashes_and_limits() {
        let mut out = Vec::new();
        let mut seen = 0;
        let (sha, len) = copy_hashed(&b"abc"[..], &mut out, 3, |n| seen = n).unwrap();
        assert_eq!(sha, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!((len, seen, out.as_slice()), (3, 3, &b"abc"[..]));
        assert!(copy_hashed(&b"abcd"[..], Vec::new(), 3, |_| {}).is_err());
    }

    #[test]
    fn newer_compares_versions() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.1.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("v0.0.9", "0.1.0"));
        assert!(!newer("nightly", "0.1.0"));
        assert!(!newer("0.2.0", "nightly"));
        assert!(newer("1.2.1", "1.2"));
        assert!(!newer("1.2", "1.2.0"));
        assert!(newer("V2", "1.9.9-beta+3"));
        assert!(!newer("1..2", "1.0"));
        assert!(!newer("", "1.0"));
        assert!(!newer("1.0", ""));
        assert!(!newer("1.a", "1.0"));
        assert!(!newer("99999999999999999999999", "1"));
    }

    #[test]
    fn installed_checks_the_name() {
        for name in ["Jay-C/..", "..\\Jay-C", "Jay C", ""] {
            assert!(installed(name).is_err(), "{name:?}");
        }
        assert_eq!(installed("Jay-I-does-not-exist-test"), Ok(None));
    }

    #[test]
    fn run_builds_the_command() {
        let path = Path::new("setup.exe");
        let args = |mode| run(path, mode).get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(run(path, Mode::Interactive).get_program(), path.as_os_str());
        assert!(args(Mode::Interactive).is_empty());
        assert_eq!(args(Mode::Update { relaunch: false }), ["/P", "/UPDATE"]);
        assert_eq!(args(Mode::Update { relaunch: true }), ["/P", "/UPDATE", "/R"]);
    }

    fn jay_dirs() -> usize {
        fs::read_dir(std::env::temp_dir()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("jay-i-")).count()
    }

    fn by_hand(url: &str, name: &str, size: u64) -> Installer {
        Installer { name: name.into(), url: url.into(), size, sha256: "0".repeat(64) }
    }

    #[test]
    fn download_fails_offline_and_leaves_nothing() {
        let _lock = DIRS.lock().unwrap_or_else(|e| e.into_inner());
        let before = jay_dirs();
        // Refused before any connection, not by a failed one.
        let http = by_hand("http://github.com/x/y/releases/download/v1/a_x64-setup.exe", "a_x64-setup.exe", 10);
        let err = download(&http, "Jay-I test", |_, _| {}).unwrap_err();
        assert!(err.contains("not an https address"), "{err}");
        for installer in [
            by_hand("http://github.com/x/y/releases/download/v1/a_x64-setup.exe", "a_x64-setup.exe", 10),
            by_hand("https://nonexistent.invalid/a_x64-setup.exe", "a_x64-setup.exe", 10),
            by_hand("https://github.com/x", "..\\a_x64-setup.exe", 10),
            by_hand("https://github.com/x", "C:a_x64-setup.exe", 10),
            by_hand("https://github.com/x", "a_x64-setup.exe", MAX_INSTALLER + 1),
            by_hand("https://github.com/x", "..", 10),
        ] {
            assert!(download(&installer, "Jay-I test", |_, _| {}).is_err(), "{installer:?}");
        }
        assert_eq!(jay_dirs(), before);
    }

    fn installer_for(bytes: &[u8]) -> Installer {
        let sha256 = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        Installer { name: "a_x64-setup.exe".into(), url: String::new(), size: bytes.len() as u64, sha256 }
    }

    #[test]
    fn store_saves_a_good_body_and_cleans_up_bad_ones() {
        let _lock = DIRS.lock().unwrap_or_else(|e| e.into_inner());
        let good = vec![7u8; 200_000];
        let installer = installer_for(&good);

        let dir = fresh_dir();
        let mut last = (0, 0);
        let path = store(&dir, &good[..], &installer, |d, t| last = (d, t)).unwrap();
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(fs::read(&path).unwrap(), good);
        assert_eq!(last, (200_000, 200_000));
        // The folder must be new.
        assert!(store(&dir, &good[..], &installer, |_, _| {}).is_err());
        assert!(path.exists(), "a failed store must not touch an existing folder's file");
        fs::remove_dir_all(&dir).unwrap();

        let mut longer = good.clone();
        longer.push(0);
        let mut wrong = good.clone();
        wrong[0] ^= 1;
        let shorter = &good[..good.len() - 1];
        for body in [&longer[..], &wrong[..], shorter] {
            let dir = fresh_dir();
            assert!(store(&dir, body, &installer, |_, _| {}).is_err());
            assert!(!dir.exists());
        }
        assert_ne!(fresh_dir(), fresh_dir());
    }

    /// The real thing, without running it: `cargo test -- --ignored` (downloads Jay-C's installer).
    #[test]
    #[ignore]
    fn downloads_and_checks_jay_c() {
        let ua = "Jay-I test";
        let release = latest("TyxMatty/Jay-C", ua).unwrap().expect("a release");
        let installer = pick_installer("TyxMatty/Jay-C", &release).unwrap();
        let mut calls = 0;
        let path = download(&installer, ua, |_, _| calls += 1).unwrap();
        let bytes = fs::read(&path).unwrap();
        let sha: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(bytes.len() as u64, installer.size);
        assert!(installer.size > 1_000_000 && calls > 0, "{} bytes, {calls} progress calls", installer.size);
        assert_eq!(sha, installer.sha256);
        println!("{}: {} ({} bytes), SHA-256 {sha} matches", release.tag_name, installer.name, installer.size);
    }
}
