use jay_i_setup::run_payload;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::{fs, thread};

const STUB: &str = env!("CARGO_BIN_EXE_stub");

/// run_payload has a global busy flag and the stub reads process-wide env vars: one test at a time.
static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn stub_bytes() -> Vec<u8> {
    fs::read(STUB).unwrap()
}

fn out_file(tag: &str) -> PathBuf {
    let out = std::env::temp_dir().join(format!("jay-i-test-{}-{tag}.txt", std::process::id()));
    let _ = fs::remove_file(&out);
    unsafe { std::env::set_var("JAYI_STUB_OUT", &out) };
    out
}

fn reset_env() {
    for v in ["JAYI_STUB_OUT", "JAYI_STUB_SLEEP_MS", "JAYI_STUB_CODE"] {
        unsafe { std::env::remove_var(v) };
    }
}

/// Setup folders left in temp by this process.
fn leftovers() -> Vec<PathBuf> {
    let prefix = format!("jay-i-setup-{}-", std::process::id());
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect()
}

#[test]
fn exit_code_and_args_pass_through_and_folder_is_removed() {
    let _g = lock();
    for code in [0, 7] {
        let out = out_file("pass");
        unsafe { std::env::set_var("JAYI_STUB_CODE", code.to_string()) };
        let got = run_payload(&stub_bytes(), "Jay-X_1.0.0_x64-setup.exe", &["/S", "/UPDATE", "/R"]).unwrap();
        assert_eq!(got, code);
        let text = fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(&lines[1..], ["/S", "/UPDATE", "/R"]);
        assert!(lines[0].ends_with("Jay-X_1.0.0_x64-setup.exe"), "{}", lines[0]);
        assert!(leftovers().is_empty(), "{:?}", leftovers());
        reset_env();
    }
}

#[test]
fn failures_clean_up_too() {
    let _g = lock();
    reset_env();
    assert!(run_payload(&[], "a.exe", &[]).is_err());
    for bad in ["", "..", "a/b.exe", r"a\b.exe", "c:x.exe"] {
        assert!(run_payload(b"x", bad, &[]).is_err(), "{bad}");
    }
    // Not an executable: the spawn fails after the file was written.
    assert!(run_payload(b"not an exe", "bad.exe", &[]).is_err());
    assert!(leftovers().is_empty(), "{:?}", leftovers());
}

fn wait_for(path: &Path) -> String {
    let start = Instant::now();
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && text.ends_with('\n')
        {
            return text;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "stub never started");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn running_payload_is_locked_and_a_second_run_is_refused() {
    let _g = lock();
    let out = out_file("lock");
    unsafe { std::env::set_var("JAYI_STUB_SLEEP_MS", "1500") };
    let bytes = stub_bytes();
    let runner = {
        let bytes = bytes.clone();
        thread::spawn(move || run_payload(&bytes, "locked.exe", &[]))
    };
    let exe = PathBuf::from(wait_for(&out).lines().next().unwrap());
    let dir = exe.parent().unwrap().to_path_buf();

    assert!(fs::write(&exe, b"evil").is_err(), "write");
    assert!(fs::OpenOptions::new().write(true).open(&exe).is_err(), "open for write");
    assert!(fs::remove_file(&exe).is_err(), "delete");
    assert!(fs::rename(&exe, dir.join("other.exe")).is_err(), "rename");
    let renamed_dir = fs::rename(&dir, dir.with_file_name(format!("{}-moved", dir.file_name().unwrap().to_string_lossy())));
    println!("renaming the folder of the running payload succeeded: {}", renamed_dir.is_ok());
    assert!(renamed_dir.is_err(), "renaming the folder of the held file succeeded");

    let second = run_payload(&bytes, "second.exe", &[]);
    assert_eq!(second.unwrap_err(), "another install is already running");

    assert_eq!(runner.join().unwrap(), Ok(0));
    assert!(leftovers().is_empty(), "{:?}", leftovers());
    reset_env();
}
