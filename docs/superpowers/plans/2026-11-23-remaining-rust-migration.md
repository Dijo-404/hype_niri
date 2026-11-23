# Remaining Rust Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Track completed steps in the execution ledger.

**Goal:** Replace the remaining shell-owned behavior with Rust and retain one small source-building `curl | bash` bootstrap.

**Architecture:** Extend `hype-control` for desktop helpers. Add GTK-independent `hype-install` for the complete installer, with separate files for core/package flow, configuration publication and system setup. Keep privileged operations at explicit process boundaries, and exercise filesystem changes in temporary roots.

**Tech Stack:** Rust 2024, existing workspace libraries, std filesystem/process APIs, libc, tempfile; Bash only for bootstrap.

**Spec:** `docs/superpowers/specs/2026-11-23-remaining-rust-migration-design.md`

## Global Constraints

- Build locally; no release pipeline or downloads of unverified executables.
- Preserve installer phases, confirmations, backups, package/provider behavior, service ordering and optional setup behavior.
- `hype-install` must build without GTK, hype-desktop, Niri parser or wallpaper libraries.
- Exactly one tracked `.sh` remains: `install.sh`.
- Never embed or dispatch the previous shell installer; preserve executable argument boundaries.
- Do not install, alter live settings, push, merge, publish or reboot during verification.
- Continue on the existing `feature/niri-notch` branch in the user-selected workspace; do not relocate their active checkout.

## Review Focus

- Piped shell source must never supply confirmation answers; use a terminal and fail clearly without one.
- Source/build failure must not replace configuration or discard a user's modified checkout.
- Existing symlink targets and saved monitor/wallpaper state must survive reinstall and failed publication.
- AUR/provider resolution and terminal failures must stop before a partial package installation.
- Privileged Stealth migration must validate first and restore previous files/services after a failed mutation.

## Shared interfaces

`installer/src/lib.rs` exports `Result<T>`, `CommandOutput { code: i32, stdout: String, stderr: String }`, `Runner` and `Installer`.

`Runner` has `output(&self, program: &str, args: &[&str]) -> Result<CommandOutput>`, `run(&self, program: &str, args: &[&str]) -> Result<()>`, `confirm(&self, prompt: &str) -> Result<bool>`, `input(&self, prompt: &str) -> Result<String>` and `available(&self, program: &str) -> bool`. Production `run` inherits terminal streams; tests supply a runner with controlled external boundaries.

`Installer` has public `source: PathBuf`, `home: PathBuf`, `root: PathBuf`, `backup: Option<PathBuf>`, `runner: Box<dyn Runner>`, and `build_dir: Option<tempfile::TempDir>`. Its helpers are `user(&self, relative: &str) -> PathBuf`, `system(&self, absolute: &str) -> PathBuf`, `confirm(&self, prompt: &str) -> Result<bool>`, `command(&self, program: &str, args: &[&str]) -> Result<()>`, `output(&self, program: &str, args: &[&str]) -> Result<CommandOutput>`, and `write_system(&self, absolute: &str, contents: &[u8], mode: u32) -> Result<()>`. Root defaults to `/`; alternate roots are constructed in Rust tests, never accepted from untrusted privileged CLI arguments.

Configuration module provides methods `backup_configs`, `copy_configs`, `setup_lazyvim`, `setup_shell`, `setup_gtk`, `setup_desktop_integrations` and `setup_user_dirs`, all `(&mut self) -> Result<()>`. `build_programs(&mut self) -> Result<PathBuf>` builds the workspace once and returns the release directory. It is shared with optional Stealth staging.

System module provides methods `setup_system`, `setup_logind`, `setup_firewall`, `setup_cloudflare`, `setup_stealth`, `validate`, all `(&mut self) -> Result<()>`, plus `setup_oomd(&mut self) -> Result<()>` and `install_stealth(&mut self, payload: &Path) -> Result<()>` for focused elevated setup.

Desktop helper module provides `pub async fn run(command: &str, arguments: &[String]) -> hype_desktop::Result<()>`, dispatched by `native/src/main.rs`.

### Task 1: Installer core and package flow

**Files:** Create `installer/Cargo.toml`, `installer/src/{lib,main,packages}.rs`; modify workspace manifest/lockfile.

- [x] Add GTK-free package and shared interfaces; write package classification and fail-before-install tests first.
- [x] Verify missing behavior fails, then implement preflight, locks, prompts, argument handling, mirror refresh, keyring/full upgrade, package installation and diagnostic logs.
- [x] Preserve exact AUR targets, installed provider retention, required upgrade refusal and failed phase propagation.
- [x] Run `cargo test -p hype-install`; expected all core/package tests pass, with no GTK compilation.

### Task 2: Configuration publication and desktop integration

**Files:** Create `installer/src/config.rs`, configuration tests. Own no shared manifests or core files.

- [x] Read previous installer configuration functions and their Python regressions; write tests for symlinks, failed publication, monitor state, user directories, browser defaults, LazyVim and Bluetooth handling.
- [x] Observe missing behavior failures, then port all configuration functions behind shared `Installer` methods.
- [x] Stage validated binaries before caller replacement. Preserve mixed installed versions through compatibility entry points during publication, then remove obsolete paths.
- [x] Run focused configuration tests and report preserved regression coverage.

### Task 3: System setup, Stealth installation and validation

**Files:** Create `installer/src/system.rs`, system/Stealth tests. Own no shared manifests or core files.

- [x] Read prior installer system functions, both standalone setup scripts and their regressions; write failure/rollback tests first.
- [x] Port required/optional services, Docker, display manager, OOM, lid, UFW, WARP and Stealth setup, retaining ordering and all options.
- [x] Expose focused OOM and Stealth installation entry points. Validate source, executable/version, Tor/nft/sudoers before publication; retain backups and rollback.
- [x] Port final validation. Run focused system and Stealth tests; no live service/network mutation.

### Task 4: Desktop helper migration

**Files:** Create `native/src/helpers.rs`; modify `native/src/{lib,main}.rs`, Niri/Waybar callers and launcher/WARP Python tests. Remove eight `scripts/*.sh` helpers only after replacements work.

- [x] Port regression harnesses to actual `hype-control` calls; pin singleton startup, toggle serialization, argument forwarding, password cleanup and bounded WARP status.
- [x] Observe failures, then implement direct IPC/D-Bus/process/filesystem behavior, preserving external application boundaries.
- [x] Update all desktop callers together, including shell expressions inside clipboard/emoji bindings.
- [x] Run helper tests, native tests and isolated GTK checks. Expected all pass without old runtime scripts.

### Task 5: Bootstrap, test entry points and final integration

**Files:** Replace `install.sh`; add bootstrap tests; update installer regression suite and docs; remove obsolete setup/test `.sh` files.

- [x] Write executable bootstrap tests for pipe/local execution, tty input, arguments, missing tools, platform rejection and failure preservation; observe failures against old installer.
- [x] Implement small bootstrap: Arch/root/tool checks, normal-user source acquisition, terminal handoff, GTK-free installer build, then execution. Preserve failure logs and clean temporary source only after the installer exits.
- [x] Replace shell test entry points with Rust integration tests/Cargo commands. Retain meaningful previous assertions; do not mark removed coverage as passing.
- [x] Update README/INSTALL/check commands and approved spec status. Verify only `install.sh` remains as a tracked shell file.
- [x] Run workspace tests/builds, all Python suites, GTK UI checks, Clippy, rustfmt, Ruff, Prettier, ShellCheck/shfmt, Niri/systemd validation and whitespace checks.
- [x] Obtain independent review of the complete change; fix material findings with failing regression proof, then repeat affected checks.
- [x] Commit verified changes as Dijo-404 on the existing branch. Do not push or deploy.
