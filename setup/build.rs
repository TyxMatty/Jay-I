use std::{env, fs, path::PathBuf};

fn var(name: &str, default: &str) -> String {
    println!("cargo:rerun-if-env-changed={name}");
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    // No payload: an empty file, so `cargo test/clippy` build; the setup then shows its Error screen.
    println!("cargo:rerun-if-env-changed=JAYI_PAYLOAD");
    let payload = match env::var("JAYI_PAYLOAD") {
        Ok(p) if !p.is_empty() => {
            println!("cargo:rerun-if-changed={p}");
            p
        }
        _ => {
            let empty = out.join("empty-payload");
            fs::write(&empty, b"").unwrap();
            empty.to_string_lossy().into_owned()
        }
    };
    println!("cargo:rustc-env=JAYI_PAYLOAD_PATH={payload}");

    let name = var("JAYI_NAME", "Jay-Test");
    assert!(
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "JAYI_NAME must be ASCII letters, digits and '-'"
    );
    println!("cargo:rustc-env=JAYI_NAME={name}");
    println!("cargo:rustc-env=JAYI_VERSION={}", var("JAYI_VERSION", "0.0.0"));

    let exe = var("JAYI_EXE", "jay-test.exe");
    assert!(
        exe.to_ascii_lowercase().ends_with(".exe") && !exe.contains(['/', '\\', ':']),
        "JAYI_EXE must be a plain file name ending in .exe"
    );
    println!("cargo:rustc-env=JAYI_EXE={exe}");

    println!("cargo:rerun-if-env-changed=JAYI_MARK");
    let mark = match env::var("JAYI_MARK") {
        Ok(p) if !p.is_empty() => p,
        _ => PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("icons/mark.png").to_string_lossy().into_owned(),
    };
    println!("cargo:rerun-if-changed={mark}");
    println!("cargo:rustc-env=JAYI_MARK_PATH={mark}");

    // Static imports load only from System32: a setup run from Downloads can't be fed a planted DLL.
    println!("cargo:rustc-link-arg-bin=jay-i-setup=/DEPENDENTLOADFLAG:0x800");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    tauri_build::build();
}
