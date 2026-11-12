# Zsh Aliases

Aliases to speed up your terminal workflow in Hype Niri.

## Navigation & Listing

- `c`, `cl`: Clear screen
- `ls`: Modern list with icons (`eza` if installed, falls back to `ls --color=auto`)
- `l`, `ll`: Long list format
- `la`, `lsa`: List all files (including hidden)
- `lt`: Sort by modification time
- `lS`: Sort by file size
- `ldir`: List directories only
- `tree`: Tree view hierarchy (`eza --tree`)
- `..`, `...`, `....`: Go up one, two, or three directories
- `~`: Go to home directory
- `-`: Go to previous directory (via `cd -`)

## Safety Nets (Interactive execution)

- `cp`: Interactive & verbose copy (`cp -iv`)
- `mv`: Interactive & verbose move (`mv -iv`)
- `rm`: Interactive & verbose remove (`rm -Iv`)
- `ln`: Interactive & verbose link (`ln -iv`)
- `mkdir`: Create parent directories implicitly (`mkdir -pv`)

## Search & Grep

- `grep`: Colorized `grep`
- `fgrep`: Colorized fixed-string search (`grep -F`)
- `egrep`: Colorized extended-regex search (`grep -E`)

## Git

- `g`: `git`
- `gs`: `git status`
- `ga`: `git add`
- `gaa`: `git add .`
- `gc`: `git commit`
- `gcm`: `git commit -m`
- `gp`: `git push`
- `gpl`: `git pull`
- `gd`: `git diff`
- `gl`: `git log --oneline --graph --decorate`
- `gco`: `git checkout`
- `gb`: `git branch`

## Package Management (Yay)

- `syu`: Upgrade official packages, then AUR packages (`sudo pacman -Syu && yay -Syu`)
- `install`: Install package (`yay -S`)
- `remove`: Remove package & unused deps (`yay -Rns`)
- `search`: Search for package (`yay -Ss`)
- `pkginfo`: Package information (`yay -Qi`)

## Docker

- `d`: `docker`
- `dc`: `docker compose`
- `dps`: `docker ps`
- `dpsa`: `docker ps -a`
- `di`: `docker images`
- `dex`: `docker exec -it`
- `drm`: `docker rm`
- `drmi`: `docker rmi`

## Editors & CLI Tools

- `v`, `vim`, `nv`, `e`: `nvim`
- `lg`: `lazygit`
- `ld`: `lazydocker`
- `h`: `history`
- `j`: `jobs -l`
- `wget`: Resume capable wget (`wget -c`)
- `path`: Print `$PATH` variables on new lines
- `cat`: `bat` with syntax highlighting (if installed)
- `catp`: `bat --style=plain` (no line numbers)

## Config Shortcuts

- `zshrc`: Edit `~/.zshrc` in Neovim
- `niriconf`: Edit Niri config in Neovim
- `alacrittyconf`: Edit Alacritty config in Neovim
- `reload`: Source `~/.zshrc` to apply changes instantly

## System Utilities

- `df`: Human-readable disk usage
- `du`: Human-readable file usage
- `free`: Human-readable memory usage
- `psg`: Search process list visually
- `ports`: Show listening TCP and UDP sockets (`ss -tulnp`)
- `ff`: Run `fastfetch` system info

## Stealth (Tor)

- `stealth-start`: Start transparent Tor routing and verify a normal TCP request uses Tor
- `stealth-status`: Show service, firewall, MAC, and live Tor verification
- `stealth-stop`: Restore the original Wi-Fi MAC and connection when needed, then remove the Stealth firewall

These are Zsh functions loaded from `~/.local/share/stealth/stealth.zsh`. See [installation and traffic limits](INSTALL.md#stealth-tor-routing).
