//! Fake payload for the tests (never shipped): records how it was started, then exits as told.
use std::{env, fs, thread, time::Duration};

fn main() {
    if let Some(out) = env::var_os("JAYI_STUB_OUT") {
        let mut text = format!("{}\n", env::current_exe().unwrap().display());
        for arg in env::args().skip(1) {
            text.push_str(&arg);
            text.push('\n');
        }
        fs::write(out, text).unwrap();
    }
    let ms = env::var("JAYI_STUB_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    thread::sleep(Duration::from_millis(ms));
    std::process::exit(env::var("JAYI_STUB_CODE").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
}
