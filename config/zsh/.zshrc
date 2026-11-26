typeset -g POWERLEVEL9K_INSTANT_PROMPT=off

[[ $- != *i* ]] && return


alias c='clear'
alias cl='clear'

alias ls='ls --color=auto'
alias l='ls -lh'
alias la='ls -lAh'
alias ll='ls -lh'
alias lsa='ls -lah'
alias lt='ls -lhtr'
alias lS='ls -lhS'
alias ldir='ls -lhd */'

alias ..='cd ..'
alias ...='cd ../..'
alias ....='cd ../../..'
alias ~='cd ~'
alias -- -='cd -'

alias grep='grep --color=auto'
alias fgrep='grep -F'
alias egrep='grep -E'

alias cp='cp -iv'
alias mv='mv -iv'
alias rm='rm -Iv'
alias ln='ln -iv'
alias mkdir='mkdir -pv'

alias g='git'
alias gs='git status'
alias ga='git add'
alias gaa='git add .'
alias gc='git commit'
alias gcm='git commit -m'
alias gp='git push'
alias gpl='git pull'
alias gd='git diff'
alias gl='git log --oneline --graph --decorate'
alias gco='git checkout'
alias gb='git branch'

alias df='df -h'
alias du='du -h'
alias free='free -h'
alias psg='ps aux | grep -v grep | grep -i -e VSZ -e'
alias ports='ss -tulnp'

alias install='yay -S'
alias remove='yay -Rns'
alias search='yay -Ss'
alias pkginfo='yay -Qi'

syu() {
    sudo pacman -Syu && yay -Syu
}

alias v='nvim'
alias vim='nvim'
alias nv='nvim'
alias e='nvim'

alias lg='lazygit'
alias ld='lazydocker'
alias h='history'
alias j='jobs -l'
alias path='echo "$PATH" | tr ":" "\n"'

alias d='docker'
alias dc='docker compose'
alias dps='docker ps'
alias dpsa='docker ps -a'
alias di='docker images'
alias dex='docker exec -it'
alias drm='docker rm'
alias drmi='docker rmi'

alias zshrc='nvim ~/.zshrc'
alias niriconf='nvim ~/.config/niri/config.kdl'
alias alacrittyconf='nvim ~/.config/alacritty/alacritty.toml'

alias wget='wget -c'
alias ff='fastfetch'
alias reload='source ~/.zshrc'


if [[ -f /usr/share/fzf/key-bindings.zsh ]]; then
    source /usr/share/fzf/key-bindings.zsh
fi
if [[ -f /usr/share/fzf/completion.zsh ]]; then
    source /usr/share/fzf/completion.zsh
fi
export FZF_DEFAULT_OPTS="--height 40% --layout=reverse --border --inline-info"
export FZF_DEFAULT_COMMAND='fd --type f --hidden --follow --exclude .git'
export FZF_CTRL_T_COMMAND="$FZF_DEFAULT_COMMAND"

if command -v zoxide &> /dev/null; then
    eval "$(zoxide init zsh)"
fi

if command -v eza &> /dev/null; then
    alias ls='eza --color=auto --icons'
    alias l='eza -lh --icons'
    alias la='eza -lah --icons'
    alias ll='eza -lh --icons'
    alias lsa='eza -lah --icons'
    alias lt='eza -lh --icons --sort=modified'
    alias lS='eza -lh --icons --sort=size'
    alias tree='eza --tree --icons'
fi

if command -v bat &> /dev/null; then
    alias cat='bat --style=auto'
    alias catp='bat --style=plain'
    export BAT_THEME="ansi"
    export MANPAGER="sh -c 'col -bx | bat -l man -p'"
fi

if [ -d "$HOME/miniconda3" ]; then
    __conda_setup="$("$HOME/miniconda3/bin/conda" 'shell.zsh' 'hook' 2> /dev/null)"
    if [ $? -eq 0 ]; then
        eval "$__conda_setup"
    elif [ -f "$HOME/miniconda3/etc/profile.d/conda.sh" ]; then
        . "$HOME/miniconda3/etc/profile.d/conda.sh"
    else
        export PATH="$HOME/miniconda3/bin:$PATH"
    fi
    unset __conda_setup
fi


HISTFILE=~/.zsh_history
HISTSIZE=10000
SAVEHIST=10000
setopt HIST_IGNORE_DUPS
setopt HIST_IGNORE_ALL_DUPS
setopt HIST_FIND_NO_DUPS
setopt APPEND_HISTORY
setopt SHARE_HISTORY

autoload -Uz compinit
mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}/zsh"
compinit -i -d "${XDG_CACHE_HOME:-$HOME/.cache}/zsh/zcompdump"
zstyle ':completion:*' matcher-list 'm:{a-z}={A-Z}'

for fzf_tab_plugin in \
    /usr/share/zsh/plugins/fzf-tab/fzf-tab.plugin.zsh \
    ~/.zsh/fzf-tab/fzf-tab.plugin.zsh; do
    [[ -f "$fzf_tab_plugin" ]] && source "$fzf_tab_plugin" && break
done
unset fzf_tab_plugin
zstyle ':completion:*:git-checkout:*' sort false
zstyle ':completion:*' list-colors ${(s.:.)LS_COLORS}
zstyle ':completion:*' menu no
zstyle ':fzf-tab:complete:cd:*' fzf-preview 'eza -1 --color=always $realpath'
zstyle ':fzf-tab:complete:__zoxide_z:*' fzf-preview 'eza -1 --color=always $realpath'

autoload -Uz colors && colors


[[ -f /usr/share/zsh-theme-powerlevel10k/powerlevel10k.zsh-theme ]] && \
    source /usr/share/zsh-theme-powerlevel10k/powerlevel10k.zsh-theme

if [[ -z "${HYPE_NIRI_FASTFETCH_SHOWN:-}" && -t 1 ]] && command -v fastfetch >/dev/null; then
    export HYPE_NIRI_FASTFETCH_SHOWN=1
    fastfetch
fi


[[ -f /usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh ]] && \
    source /usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh

[[ -f /usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh ]] && \
    source /usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh

[[ ! -f ~/.p10k.zsh ]] || source ~/.p10k.zsh

export ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE="fg=245"


[[ -f "$HOME/.local/bin/env" ]] && . "$HOME/.local/bin/env"
[[ ":$PATH:" == *":$HOME/bin:"* ]] || export PATH="$HOME/bin:$PATH"


[[ -d "$HOME/.opencode/bin" ]] && export PATH="$HOME/.opencode/bin:$PATH"
[[ -d "$HOME/.npm-global/bin" ]] && export PATH="$HOME/.npm-global/bin:$PATH"

if [[ -d "$HOME/.bun" ]]; then
    export BUN_INSTALL="$HOME/.bun"
    export PATH="$BUN_INSTALL/bin:$PATH"
    [ -s "$HOME/.bun/_bun" ] && source "$HOME/.bun/_bun"
fi

[[ -f "$HOME/.local/share/stealth/stealth.zsh" ]] && \
    source "$HOME/.local/share/stealth/stealth.zsh"

backward-delete-one-word() {
    local WORDCHARS=''
    zle backward-kill-word
}
zle -N backward-delete-one-word
bindkey -M emacs $'\e\x7f' backward-delete-one-word
bindkey -M viins $'\e\x7f' backward-delete-one-word
