# Remaining Rust migration and one-command installation

Status: implemented and verified after user approval.

## Intent and decisions

Move the remaining shell-owned desktop and installation behavior into Rust. Keep
one small `install.sh` because the requested entry point is `curl ... | bash`.
The user selected local Rust builds rather than prebuilt release downloads.

Proposed public command, after the change reaches the repository's main branch:

```bash
curl -fsSL https://raw.githubusercontent.com/Dijo-404/hype_niri/main/install.sh | bash
```

Preparing this command does not authorize publishing, pushing, installing on the
current machine, changing its network, or rebooting it.

## Approaches

1. **Local build, selected by the user.** Download source and build an installer
   that does not depend on GTK. That installer installs desktop dependencies and
   builds the existing workspace. This needs no release publishing infrastructure
   but costs compilation time on the target machine.
2. **Prebuilt releases.** Download architecture-specific installer and desktop
   artifacts. Faster startup, but requires releases, checksums and a maintained
   architecture matrix. The user did not select this approach.

Do not embed or invoke the old Bash installer from Rust. External applications
remain applications: Rust may invoke Cargo, Git, pacman, yay, sudo, Wofi and
vendor tools where those tools own the operation. Native filesystem, process,
JSON and configuration work replaces shell pipelines. Existing direct Niri and
D-Bus APIs remain the preferred desktop interfaces.

## Bootstrap

`install.sh` becomes the only tracked `.sh` file. Its responsibility is obtaining
source, building the Rust installer and handing off execution. It has no desktop
configuration phases.

- Support execution from a checkout and from a pipe. Avoid relying on
  `BASH_SOURCE[0]` when the script has no local pathname.
- Preserve invocation arguments when handing off to Rust.
- Reopen `/dev/tty` for prompts and child-process input. Do not read installation
  answers from the pipe containing shell source. Fail clearly without a terminal.
- Reject a root invocation; elevate individual required operations.
- Reuse a checkout when invoked locally. A downloaded source tree must not
  replace a user's modified checkout or use a destructive Git reset.
- Download into a temporary location and stop on download, checkout or build
  failure. Do not configure the desktop after such a failure.
- Check Arch and bootstrap requirements. If Git, Cargo or compilation tools are
  missing, explain and confirm the required `pacman -Syu --needed` operation
  before installing them. Do not introduce a partial Arch upgrade.
- Keep the current requirement for an existing yay installation. Do not quietly
  add AUR-helper bootstrapping.

The default download source is the actual project remote, not the GREMLO example.
The public main-branch command becomes available only after this work is published.

## Rust installer

Add a separate `hype-install` workspace member with no dependency on GTK,
`hype-desktop`, Niri's parser or wallpaper libraries. Building only this package
must succeed before GTK and gtk4-layer-shell are installed. Reuse the workspace
lockfile. Keep package-manager and privilege boundaries explicit.

Port all current installation phases rather than replacing them with a reduced
installer:

- Source, platform, network and yay preflight; exclusive installer lock.
- Optional mirror refresh; required keyring and full system upgrade; official and
  AUR package classification, provider handling, diagnostics and install logs.
- Existing-config backups, configuration staging and replacement, generated-file
  handling and symlink protection.
- Desktop binary build and validation before configuration replacement; generated
  monitor settings; wallpaper preservation and initial selection; lowercase XDG
  directories and existing drive links.
- LazyVim setup, previous Neovim backups and existing LazyVim preservation.
- Zsh configuration, GTK themes, icons, fonts, MIME defaults, Bluetooth pairing
  dialogs, stale-service cleanup and file-indexing exclusions.
- Pacman settings, optional display-manager switch, Polkit, required and optional
  services, Docker socket behavior and group membership, user audio and idle units.
- OOM policy setup and validation, optional lid behavior, optional UFW defaults,
  optional WARP setup, optional Stealth installation and final validation.
- Existing confirmation behavior, phase failures, summary and optional reboot.

Native setup code absorbs `systemd/setup-oomd.sh` and
`stealth/install-stealth.sh`. Preserve Stealth's executable/version checks,
operation lock, inactive-service checks, configuration validators, backups,
rollback, legacy migration, permissions and reinstall behavior. Retain the current
rule that installation leaves Stealth inactive.

Keep package installation and AUR builds under the normal user/explicit sudo
boundary. Do not run the entire installer as root. Keep privileged file operations
small and reviewable; do not evaluate generated shell commands.

## Desktop helpers

Extend the existing `hype-control` command boundary and shared services:

| Existing script         | Rust replacement behavior                                                                                          |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `fullscreen-toggle.sh`  | Fullscreen through existing Niri IPC.                                                                              |
| `menu-toggle.sh`        | Preserve Wofi toggle, serialization and argument forwarding.                                                       |
| `password-prompt.sh`    | Preserve password masking and caller arguments.                                                                    |
| `power-menu.sh`         | Preserve choices, caffeine cleanup and lock/suspend readiness; use Niri and logind interfaces for session actions. |
| `start-tray-applets.sh` | Preserve user-scoped singleton locks, optional app availability, inherited environment and child lifetime.         |
| `start-waybar.sh`       | Preserve custom config path, theme, singleton behavior and JSONC pass-through.                                     |
| `stealth-indicator.sh`  | Preserve image/tooltip output and legacy-state compatibility.                                                      |
| `warp-indicator.sh`     | Preserve hiding with the native tray, bounded status lookup, connected classification and JSON escaping.           |

WARP's existing vendor `warp-cli` boundary may remain a direct executable call
from Rust. Claiming a verified native daemon protocol is outside the evidence
currently available; do not invent an undocumented protocol or embed shell code.

Update Niri, Waybar, installer validation, documentation and any remaining callers
together. Preserve the clipboard and emoji launchers' required shell expressions
inside configuration; migrating `.sh` files does not imply replacing external
applications or every shell expression in dotfiles.

## Tests and transition

Replace `tests/test-notch.sh`, `stealth/test-stealth.sh` and
`stealth/test-install-stealth.sh` with Rust tests or Cargo entry points. Preserve
their meaningful checks, including Stealth installer failure and rollback cases.
Update Python launcher/installer tests to exercise Rust boundaries or port the
equivalent scenarios into Rust; do not discard behavior coverage because Bash
functions disappear.

Cover piped bootstrap input, local-checkout execution, argument forwarding,
missing tools, rejected platform, absent terminal, failed download/build and
preservation of existing files. Use temporary files, fake command boundaries and
private buses/namespaces. Exercise the real Rust binaries where practical.

The installer must preserve operation during the transition from installed shell
bindings to Rust bindings. Stage runnable replacements before replacing their
callers. Retire compatibility paths only after their corresponding configuration
has been published. A failure retains a usable prior configuration or identifies
the exact backup needed for recovery.

## Acceptance and limits

- Exactly one tracked `.sh` file remains: the small bootstrap `install.sh`.
- No old shell installer is embedded, generated or dispatched by Rust.
- Local checkout and piped bootstrap paths reach the same Rust installer.
- All existing setup options and required failure/backup behavior remain covered.
- Existing native controls, notch and their callers continue to pass regression
  tests, including isolated GTK checks.
- Rust format, Clippy, tests and release builds pass; Ruff, Prettier, remaining
  shell checks, configuration validation and whitespace checks pass.
- Documentation explains prerequisites, local build time and the proposed public
  installation command without claiming unpublished code is already available.

No live system installation, physical device operations, network routing changes,
push, merge or release publication is part of implementation verification.
