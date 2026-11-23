# Minimal desktop and native utility migration

## Intent

Reduce the applications, helper processes, packages, and services required by Hype Niri. Prefer established desktop protocols and maintained libraries. The user selected a minimal desktop with extra applications and services installed only when selected. Preserve desktop features while replacing avoidable command wrappers with Rust implementations.

## Approach

Three approaches were evaluated:

1. Keep the existing application bundle and replace a few command wrappers. This leaves most unnecessary installation and service footprint intact.
2. Use a minimal desktop, native utility features, and explicitly selected application/service groups. This is the selected approach.
3. Replace the compositor, audio/network services, locker, and other infrastructure with custom Rust implementations. This creates a large maintenance burden and conflicts with long-term reliability.

Retain Niri, GTK, Waybar, NetworkManager, PipeWire/WirePlumber, logind/systemd, Hyprlock, Hypridle, awww, Polkit and desktop portals. These provide the desktop's infrastructure. Retain vendor CLIs and configuration validators where the owning application supplies the reliable interface. WARP, Proton VPN, Docker, Bluetooth, and bundled end-user applications are optional selections.

## Native utility changes

- Replace Wofi application selection, power selection, and password UI with existing native GTK dialog infrastructure. Discover and launch installed desktop applications through GIO. Preserve keyboard selection, cancellation, single-instance toggle behavior and existing shortcuts.
- Replace playerctl key actions with direct MPRIS D-Bus control, including player disappearance and no-player handling. Reuse local patterns from the notch.
- Replace idle lock and monitor control CLI calls with native logind/Niri calls. Keep Hypridle's idle/inhibitor implementation. Preserve lock-before-suspend guarantees.
- Add native clipboard history and selection through Wayland data-control protocols. One Rust watcher replaces the two wl-paste/cliphist watchers. Preserve text and image data with correct MIME types, bounded reads/storage, private state, deduplication, and selection persistence. Keep old cliphist history untouched; automatic database conversion is outside scope.
- Replace bemoji/Wofi/wtype with a searchable native emoji picker using maintained Unicode emoji data and native Wayland virtual-keyboard text insertion. Keep typing behavior, rather than silently replacing it with copy-only behavior.
- Use a Rust clipboard-owner process when selection must survive the picker; do not launch wl-copy. Use existing runtime locks and lifecycle conventions.
- Replace the Waybar stop shell command with existing native process matching/signaling. Use direct Niri spawn for ordinary argument-vector application launches.
- Remove default Wofi, playerctl, cliphist, bemoji, wtype, wl-clipboard and obsolete wallpaper/notification/sensor CLI requirements once the native paths and tests replace their actual uses.

Use the current GTK/GIO, D-Bus, filesystem and runtime utilities. Candidate additions are wl-clipboard-rs, the maintained Wayland client/protocol crates, and an emoji metadata crate. Check current upstream documentation before adding them. Do not add another service manager, general plugin framework or database service.

## Minimal installation and service ownership

- Keep pkglist.txt as the minimal desktop/build requirements. Move optional applications into a small explicit package-group manifest. Offer groups before package installation; selecting none installs the core only. Avoid requiring yay when selected requirements are satisfied by official repositories or installed providers.
- Make browsers, development applications, chat/notes, media/recording, Docker, Bluetooth, VPN clients, additional shell tools, compatibility/media extras and hardware-specific drivers explicit selections. Do not install multiple browsers/editors or machine-specific Intel drivers by default.
- Configure Docker socket/group membership, Bluetooth and WARP only when selected during a full install. Existing installed packages must not accidentally opt a user into their service setup. Keep manually requested installer phases usable.
- Keep essential audio, network, automatic power and OOM services. Optional applets launch only when the corresponding integration is enabled. Do not stop existing live services or uninstall packages as part of this source change.
- Make LazyVim setup, browser defaults, font/theme extras and final validation consistent with selected packages. A minimal install must not fail because optional browsers, editors, recording tools, Bluetooth tools or removed helper CLIs are absent.
- Keep package managers, Cargo, Git, privileged file installation and authoritative Tor/sudoers/systemd configuration validators. Avoid reimplementing these tools merely to claim zero subprocess calls.
- Add a documented dependency ledger covering removed defaults, selected groups, required services and the remaining external process boundaries. Count direct package requirements separately from transitive package dependencies.

## Compatibility and boundaries

Maintain old command basenames where installed/custom configurations still use them. Native internal call sites must not depend on old shell pipelines. Preserve existing comments unless their code or documented behavior changes. Retain installer backups, rollback, package-provider handling and explicit elevation. Changes apply to repository source and future installations; deployment to this running desktop and package removal are outside this request's implementation scope.

## Acceptance

- Core launcher, media keys, power menu, idle actions, clipboard history and emoji selection use Rust/library/protocol implementations without the replaced utility executables.
- Selecting no optional groups installs only documented core requirements and does not enable optional services.
- Selecting an optional integration installs and validates its requirements and only then configures its service or application.
- Existing package-provider behavior and installer failure/backup protections pass regression tests.
- Private D-Bus fixtures exercise media/session calls. Native unit/integration tests cover clipboard retention, MIME handling, limits, deduplication and malformed state. Isolated GUI/compositor tests cover picker behavior and clipboard/typing protocols where available.
- Workspace tests, Python regressions, Rust format/Clippy and applicable config validation pass. Report hardware and live-install limits honestly.
- No live uninstall, network/service change, reboot, push or release publication.
