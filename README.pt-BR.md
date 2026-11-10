# Jay-I

[English](README.md) · **Português**

A camada de instalação da linha Jay. O Jay-I é uma biblioteca Rust (`jay-i`, sem Tauri) que acha a última release de um app Jay no GitHub, baixa o instalador, confere e roda. Ela troca as três cópias dessa lógica que o Jay-H, o Jay-E e o Jay-C levam hoje.

Só Windows, como o resto da linha.

## Estado

| Etapa | O quê | Estado |
|---|---|---|
| 1 | A crate `jay-i` | feita |
| 2 | `setup.exe` com a interface Jay, em volta do instalador de cada app | a fazer |
| 3 | Cada Jay troca a própria cópia pela crate | a fazer |

## API

```rust
pub fn latest(repo: &str, user_agent: &str) -> Result<Option<Release>, String>;
pub fn pick_installer(repo: &str, release: &Release) -> Result<Installer, String>;
pub fn download(installer: &Installer, user_agent: &str, on_progress: impl FnMut(u64, u64)) -> Result<PathBuf, String>;
pub fn run(installer: &Path, mode: Mode) -> std::process::Command;
pub fn installed(name: &str) -> Result<Option<Installed>, String>;
pub fn newer(latest: &str, current: &str) -> bool;
```

Autoatualização de um app:

```rust
let ua = "Jay-E";
if let Some(release) = jay_i::latest("TyxMatty/Jay-E", ua)? {
    if jay_i::newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        let installer = jay_i::pick_installer("TyxMatty/Jay-E", &release)?;
        let path = jay_i::download(&installer, ua, |feito, total| { /* progresso */ })?;
        jay_i::run(&path, jay_i::Mode::Update { relaunch: true }).spawn().map_err(|e| e.to_string())?;
        std::process::exit(0); // o instalador troca o exe em uso
    }
}
```

O hub instalando outro app, com a janela do NSIS:

```rust
jay_i::run(&path, jay_i::Mode::Interactive).status().map_err(|e| e.to_string())?;
```

`installed("Jay-E")` lê a chave de desinstalação do usuário e devolve a versão e a pasta de instalação, ou `None`.

## Usar

O Jay-I não está no crates.io, e o repositório no GitHub ainda não foi publicado. Até lá, use uma dependência por caminho:

```toml
jay-i = { path = "../Jay-I" }
```

Com o repositório público: `jay-i = { git = "https://github.com/TyxMatty/Jay-I" }`.

## Testes

```powershell
cargo test                  # sem rede; inclui os casos adversariais
cargo test -- --ignored     # baixa o instalador de verdade do Jay-C e confere (nunca roda)
```

O desenho, as diferenças das três cópias antigas e a revisão de segurança estão em [docs/ARQUITETURA.md](docs/ARQUITETURA.md).

Licença: MIT.
