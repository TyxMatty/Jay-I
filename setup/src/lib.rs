//! The testable logic of the setup.exe: the NSIS-style flags, the WebView2 check and running the
//! embedded installer from a locked temp file.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub passive: bool,
    pub silent: bool,
    pub forward: Vec<&'static str>,
}

/// Case-insensitive exact matches, like NSIS's GetOptions; anything else is ignored, never forwarded.
pub fn parse(args: impl IntoIterator<Item = String>) -> Args {
    let mut out = Args::default();
    for arg in args {
        let upper = arg.to_ascii_uppercase();
        match upper.as_str() {
            "/P" => out.passive = true,
            "/S" => out.silent = true,
            _ => {
                if let Some(flag) = ["/UPDATE", "/R", "/NS"].into_iter().find(|f| *f == upper)
                    && !out.forward.contains(&flag)
                {
                    out.forward.push(flag);
                }
            }
        }
    }
    out
}

/// What the payload gets: `/S` whenever our window (or none) is the interface; its own window only
/// when there is no WebView2 for ours.
pub fn payload_args(args: &Args, webview2: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if args.silent || webview2 {
        out.push("/S");
    } else if args.passive {
        out.push("/P");
    }
    out.extend(&args.forward);
    out
}

/// The Evergreen runtime's `pv` value, as Microsoft documents it for detection.
pub fn webview2_installed() -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    const CLIENT: &str = r"EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\{CLIENT}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\{CLIENT}")),
        (HKEY_CURRENT_USER, format!(r"Software\Microsoft\{CLIENT}")),
    ]
    .into_iter()
    .any(|(root, path)| {
        RegKey::predef(root)
            .open_subkey_with_flags(path, KEY_READ)
            .and_then(|key| key.get_value::<String, _>("pv"))
            .is_ok_and(|pv| !pv.is_empty() && pv != "0.0.0.0")
    })
}

static BUSY: AtomicBool = AtomicBool::new(false);
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

/// True while `run_payload` runs.
pub fn busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// Writes `payload` to a fresh temp folder, runs it with `args`, waits, cleans up; returns its exit code.
pub fn run_payload(payload: &[u8], file_name: &str, args: &[&str]) -> Result<i32, String> {
    if BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("another install is already running".into());
    }
    let _busy = BusyGuard;
    if payload.is_empty() {
        return Err("this setup was built without an installer".into());
    }
    if !plain_name(file_name) {
        return Err("invalid installer file name".into());
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    // create_dir fails if the name exists, so the folder is ours alone.
    let dir = std::env::temp_dir().join(format!("jay-i-setup-{}-{nanos}-{counter}", std::process::id()));
    fs::create_dir(&dir).map_err(|e| format!("cannot create the temp folder: {e}"))?;
    let result = write_and_run(&dir, payload, file_name, args);
    for _ in 0..10 {
        if fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    result
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> String {
    move |e| format!("{what}: {e}")
}

fn write_and_run(dir: &std::path::Path, payload: &[u8], file_name: &str, args: &[&str]) -> Result<i32, String> {
    let (path, held) = write_and_hold(dir, payload, file_name)?;
    let status = Command::new(&path).args(args).current_dir(dir).status().map_err(io("cannot start the installer"))?;
    drop(held);
    Ok(status.code().unwrap_or(1))
}

/// Writes the payload and returns it opened so that nobody else can change it until the handle drops.
fn write_and_hold(dir: &std::path::Path, payload: &[u8], file_name: &str) -> Result<(PathBuf, fs::File), String> {
    let path = dir.join(file_name);

    let mut file = OpenOptions::new().write(true).create_new(true).share_mode(0).open(&path).map_err(io("cannot write the installer"))?;
    file.write_all(payload).and_then(|()| file.flush()).map_err(io("cannot write the installer"))?;
    drop(file);

    // Others may read and run it, nobody may write, delete or rename it. A held write handle would
    // instead make CreateProcess fail: the image loader opens the exe without FILE_SHARE_WRITE.
    const FILE_SHARE_READ: u32 = 1;
    let mut held = OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&path).map_err(io("cannot lock the installer"))?;
    // Checked through the locked handle, so what was verified is what stays locked.
    let mut back = Vec::with_capacity(payload.len());
    held.read_to_end(&mut back).map_err(io("cannot read the installer back"))?;
    if back != payload {
        return Err("the installer changed on disk".into());
    }
    Ok((path, held))
}

/// Starts the installed app (`exe` inside its install folder) without waiting for it.
pub fn open_app(name: &str, exe: &str) -> Result<(), String> {
    let installed = jay_i::installed(name)?.filter(|i| !i.location.is_empty()).ok_or("the app is not installed")?;
    if !plain_name(exe) {
        return Err("invalid app file name".into());
    }
    let location = PathBuf::from(installed.location);
    let path = location.join(exe);
    if !path.is_file() {
        return Err(format!("{} not found", path.display()));
    }
    Command::new(path).current_dir(location).spawn().map(|_| ()).map_err(|e| format!("cannot start the app: {e}"))
}

/// Where the app will be installed, for display: the current place if installed, else the default.
pub fn install_folder(name: &str) -> String {
    if let Ok(Some(i)) = jay_i::installed(name)
        && !i.location.is_empty()
    {
        return i.location;
    }
    let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(base).join(name).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(list: &[&str]) -> Args {
        parse(list.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parse_flags() {
        assert!(p(&["/P"]).passive);
        assert!(p(&["/S"]).silent);
        assert!(p(&["/s"]).silent && p(&["/p"]).passive);
        assert_eq!(p(&["/update", "/R", "/nS"]).forward, ["/UPDATE", "/R", "/NS"]);
        assert_eq!(p(&["/NS", "/ns", "/R", "/Ns", "/r"]).forward, ["/NS", "/R"]);
    }

    #[test]
    fn parse_ignores_the_rest() {
        let a = p(&[r"/D=C:\x", "/ARGS", "/X", "P", "", "/P ", "/UPDATES"]);
        assert_eq!(a, Args::default());
    }

    #[test]
    fn payload_args_by_mode() {
        let a = p(&["/S", "/update", "/R", "/NS"]);
        assert_eq!(payload_args(&a, true), ["/S", "/UPDATE", "/R", "/NS"]);
        assert_eq!(payload_args(&a, false), ["/S", "/UPDATE", "/R", "/NS"]);
        let a = p(&["/P", "/R"]);
        assert_eq!(payload_args(&a, true), ["/S", "/R"]);
        assert_eq!(payload_args(&a, false), ["/P", "/R"]);
        let a = p(&["/NS"]);
        assert_eq!(payload_args(&a, true), ["/S", "/NS"]);
        assert_eq!(payload_args(&a, false), ["/NS"]);
        assert_eq!(payload_args(&Args::default(), false), Vec::<&str>::new());
    }

    /// The gap before CreateProcess (the run itself is locked by Windows anyway): the held handle
    /// alone must keep the file as written.
    #[test]
    fn held_payload_cannot_be_changed_before_it_runs() {
        let dir = std::env::temp_dir().join(format!("jay-i-setup-hold-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let (path, held) = write_and_hold(&dir, b"payload", "a.exe").unwrap();
        assert!(fs::write(&path, b"evil").is_err(), "write");
        assert!(OpenOptions::new().append(true).open(&path).is_err(), "append");
        assert!(fs::remove_file(&path).is_err(), "delete");
        assert!(fs::rename(&path, dir.join("b.exe")).is_err(), "rename");
        assert!(fs::rename(&dir, dir.with_extension("moved")).is_err(), "rename the folder");
        assert_eq!(fs::read(&path).unwrap(), b"payload", "others may still read it");
        drop(held);
        fs::write(&path, b"now it may change").unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn webview2_is_installed_on_this_pc() {
        // Asserts true because the dev PC has the runtime; it only proves the registry read works.
        assert!(webview2_installed());
    }
}
