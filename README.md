# Jay-I

**English** · [Português](README.pt-BR.md)

The installer layer of the Jay line. Jay-I is a Rust library (`jay-i`, no Tauri) that finds the latest release of a Jay app on GitHub, downloads its installer, checks it, and runs it. It replaces the three copies of this logic that Jay-H, Jay-E and Jay-C carry today.

Windows only, like the rest of the line.

## Status

| Stage | What | State |
|---|---|---|
| 1 | The `jay-i` crate | done |
| 2 | `setup.exe` with the Jay UI, wrapping each app's installer | to do |
| 3 | Each Jay replaces its own copy with the crate | to do |

## API

```rust
pub fn latest(repo: &str, user_agent: &str) -> Result<Option<Release>, String>;
pub fn pick_installer(repo: &str, release: &Release) -> Result<Installer, String>;
pub fn download(installer: &Installer, user_agent: &str, on_progress: impl FnMut(u64, u64)) -> Result<PathBuf, String>;
pub fn run(installer: &Path, mode: Mode) -> std::process::Command;
pub fn installed(name: &str) -> Result<Option<Installed>, String>;
pub fn newer(latest: &str, current: &str) -> bool;
```

Self-update of an app:

```rust
let ua = "Jay-E";
if let Some(release) = jay_i::latest("TyxMatty/Jay-E", ua)? {
    if jay_i::newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        let installer = jay_i::pick_installer("TyxMatty/Jay-E", &release)?;
        let path = jay_i::download(&installer, ua, |done, total| { /* progress */ })?;
        jay_i::run(&path, jay_i::Mode::Update { relaunch: true }).spawn().map_err(|e| e.to_string())?;
        std::process::exit(0); // the installer replaces the running exe
    }
}
```

Hub installing another app, with the NSIS window:

```rust
jay_i::run(&path, jay_i::Mode::Interactive).status().map_err(|e| e.to_string())?;
```

`installed("Jay-E")` reads the per-user uninstall key and returns the version and install folder, or `None`.

## Use it

Jay-I is not on crates.io, and the GitHub repository is not published yet. Until then, use a path dependency:

```toml
jay-i = { path = "../Jay-I" }
```

Once the repository is public: `jay-i = { git = "https://github.com/TyxMatty/Jay-I" }`.

## Tests

```powershell
cargo test                  # offline; includes the adversarial cases
cargo test -- --ignored     # downloads Jay-C's real installer and checks it (never runs it)
```

The design, the differences from the three old copies and the security review are in [docs/ARQUITETURA.en.md](docs/ARQUITETURA.en.md).

License: MIT.
