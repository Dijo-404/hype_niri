# Niri Keybindings

These are the primary default keybindings for the Hype Niri environment.

> [!NOTE]  
> The "Super" key refers to the Windows, Command, or Meta key on your keyboard.

## Window Management

| Keybinding                           | Action                         | Description                                                                                |
| :----------------------------------- | :----------------------------- | :----------------------------------------------------------------------------------------- |
| `Super + Q`                          | `close-window`                 | Close active window                                                                        |
| `Ctrl + Q`                           | `close-window`                 | Close active window                                                                        |
| `Alt + F4`                           | `close-window`                 | Close active window                                                                        |
| `Super + Delete`                     | `quit`                         | Exit Niri completely                                                                       |
| `Super + W`                          | `toggle-column-tabbed-display` | Toggle tabbed column view                                                                  |
| `Super + Shift + S`                  | `opacity-toggle`               | Toggle window transparency (95%/90% or fully opaque)                                       |
| `Super + Shift + F`                  | `fullscreen-toggle`            | Toggle fullscreen with waybar animation                                                    |
| `Super + F`                          | `maximize-column`              | Maximize the current column                                                                |
| `Super + O`                          | `toggle-overview`              | Toggle window overview                                                                     |
| `Super + Space`                      | `toggle-window-floating`       | Toggle floating mode for a window                                                          |
| `Super + Shift + Space`              | `switch-focus`                 | Switch focus between floating/tiling                                                       |
| `Super + L`                          | `lock-dim`                     | Lock the screen, then power off monitors after 30s (any input wakes them)                  |
| Hold `Super`, double-press `Copilot` | `copilot-shortcut`             | Lock and turn off displays; repeat to show the password prompt. Applications keep running. |

## Focus & Navigation

| Keybinding        | Action              | Description                          |
| :---------------- | :------------------ | :----------------------------------- |
| `Super + ←` / `→` | `focus-column`      | Focus left/right column              |
| `Super + ↑` / `↓` | `focus-window`      | Focus up/down window within a column |
| `Alt + Tab`       | `focus-window-down` | Cycle focus down                     |

## Resize

| Keybinding                | Action                   | Description                    |
| :------------------------ | :----------------------- | :----------------------------- |
| `Super + Ctrl + ←` / `→`  | `set-column-width ±30`   | Resize column width coarsely   |
| `Super + Ctrl + ↑` / `↓`  | `set-window-height ±30`  | Resize window height coarsely  |
| `Super + -` / `=`         | `set-column-width ±10%`  | Adjust column width precisely  |
| `Super + Shift + -` / `=` | `set-window-height ±10%` | Adjust window height precisely |

## Move Window / Column

| Keybinding              | Action        | Description                       |
| :---------------------- | :------------ | :-------------------------------- |
| `Alt + Shift + ←` / `→` | `move-column` | Move entire column left/right     |
| `Alt + Shift + ↑` / `↓` | `move-window` | Move window up/down within column |

## Column Management

| Keybinding          | Action                 | Description                              |
| :------------------ | :--------------------- | :--------------------------------------- |
| `Super + R`         | `switch-preset-width`  | Cycle through preset column widths       |
| `Super + Shift + R` | `switch-preset-height` | Cycle through preset window heights      |
| `Super + Ctrl + R`  | `reset-window-height`  | Reset window to default height           |
| `Super + Ctrl + F`  | `expand-column`        | Expand column to consume available width |
| `Super + Shift + C` | `center-column`        | Center the active column on screen       |
| `Super + [`         | `consume/expel-left`   | Consume or expel window to the left      |
| `Super + ]`         | `consume/expel-right`  | Consume or expel window to the right     |
| `Super + ,`         | `consume-window`       | Consume window into current column       |

## Applications & Launchers

| Keybinding           | Action             | Description                                                        |
| :------------------- | :----------------- | :----------------------------------------------------------------- |
| `Super + T`          | `alacritty`        | Launch Terminal                                                    |
| `Super + E`          | `nautilus`         | Launch File Manager                                                |
| `Super + Shift + D`  | `monitor-refresh`  | View connected monitors and save refresh-rate defaults             |
| `Super + U`          | `open-drives`      | Mount/unlock known external/secondary drives, then launch Nautilus |
| `Super + C`          | `nvim`             | Launch Neovim                                                      |
| `Super + Copilot`    | `copilot-shortcut` | Open VS Code after a 400ms double-press window                     |
| `Super + B`          | `brave`            | Launch Browser (Brave, also the default web browser)               |
| `Ctrl + Shift + Esc` | `htop`             | Launch System Monitor                                              |
| `Super + A`          | `fuzzel`           | Launch Application Menu                                            |
| `Super + Tab`        | `toggle-overview`  | Open Window Overview                                               |
| `Super + .`          | `bemoji`           | Open Emoji Picker                                                  |
| `Super + V`          | `cliphist`         | Open Clipboard History List                                        |

For the Copilot shortcuts, hold Super and press Copilot once to open VS Code, or twice within 400ms to lock and turn off the displays without opening VS Code. Ordinary keyboard and mouse input leaves the displays off. Repeat Super + double Copilot to show Hyprlock's password prompt, then enter your Linux account password (normally the same password used by sudo) to unlock the desktop. A single press while locked does nothing.

This mode temporarily disables the enabled outputs, waits one second, then starts Hyprlock while the displays are off. It restores the outputs on the next double press; monitor modes and scales in the configuration remain unchanged. The script confirms Niri's session lock through logind before restoring outputs. If locking fails, it keeps the displays off and allows another double press to retry.

Sleep takes priority over this mode: after lid-close suspend or another system sleep, resume restores the saved displays and shows Hyprlock automatically. Your password is still required; another Copilot shortcut is unnecessary. Ordinary idle activity continues to leave the manual blackout in effect.

These bindings support the common `Super+Shift+F23` Copilot sequence and `Super+XF86Assistant` keyboards. Some Copilot keys send Super themselves, so Copilot alone can also activate these shortcuts.

With xkeyboard-config 2.49, the F23 key's base symbol is `XF86TouchpadOff`, even though Super+Shift produces `XF86Assistant`. Niri matches the base symbol and held modifiers, so `Super+Shift+XF86TouchpadOff` is also bound to the same handler. This keeps Copilot working after the keyboard-map update without binding the standalone touchpad-disable key.

## Audio & Brightness

| Keybinding             | Action            | Description                |
| :--------------------- | :---------------- | :------------------------- |
| `F10` / `Mute`         | `volume mute`     | Toggle Audio Mute          |
| `F11` / `Vol Down`     | `volume down`     | Decrease Volume            |
| `F12` / `Vol Up`       | `volume up`       | Increase Volume            |
| `Mic Mute`             | `mic mute`        | Toggle Microphone Mute     |
| `Bright Down`          | `brightness down` | Decrease Screen Brightness |
| `Bright Up`            | `brightness up`   | Increase Screen Brightness |
| `Play/Pause/Next/Prev` | `playerctl`       | Control Media Playback     |

## Workspaces & Monitors

| Keybinding                  | Action              | Description                       |
| :-------------------------- | :------------------ | :-------------------------------- |
| `Super + 1-0`               | `focus-workspace`   | Switch to workspace 1-10          |
| `Super + Shift + 1-0`       | `move-to-workspace` | Move window to workspace 1-10     |
| `Super + PageDown/Up`       | `focus-workspace`   | Navigate workspaces up/down       |
| `Super + Scroll`            | `focus-workspace`   | Scroll through workspaces         |
| `Super + Shift + ←/→`       | `focus-monitor`     | Focus left/right monitor          |
| `Super + Shift + Alt + ←/→` | `move-to-monitor`   | Move column to left/right monitor |

## Screenshots & Pickers

| Keybinding          | Action              | Description                   |
| :------------------ | :------------------ | :---------------------------- |
| `Print`             | `screenshot`        | Interactive region screenshot |
| `Ctrl + Print`      | `screenshot-screen` | Screenshot entire screen      |
| `Alt + Print`       | `screenshot-window` | Screenshot active window      |
| `Super + Shift + P` | `hyprpicker`        | Interactive color picker      |

## Miscellaneous

| Keybinding              | Action              | Description                                 |
| :---------------------- | :------------------ | :------------------------------------------ |
| `Super + Alt + ←` / `→` | `wallpaper random`  | Cycle random wallpaper next/prev            |
| `Super + Shift + W`     | `wallpaper select`  | Open wallpaper selector menu                |
| `Super + Alt + ↑`       | `waybar show`       | Show/restart Waybar                         |
| `Super + Alt + ↓`       | `waybar hide`       | Hide Waybar                                 |
| `Super + Esc`           | `inhibit-shortcuts` | Toggle Niri keyboard shortcut override      |
| `Power Button`          | `suspend`           | Stop caffeine mode, then suspend the system |
