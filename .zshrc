# ==========================================
#  Rust Wayland Power - ZSH Configuration
# ==========================================

# 1. Path Configuration
# We use 'typeset -U' to prevent duplicate entries in the path.
typeset -U path PATH

# Add your custom binary paths here. Order matters (first = higher priority).
path=(
    "$HOME/.cargo/bin"
    "$HOME/.pub-cache/bin"
    "$HOME/bin"
    "$HOME/.local/bin"
    "$HOME/go/bin"
    "$HOME/.npm-global/bin"
    "$HOME/.luarocks/bin"
    "$HOME/.tmux/plugins/tmuxifier/bin"
    # Ruby (Version specific - keep an eye on this if you upgrade ruby)
    "$HOME/.local/share/gem/ruby/3.4.0/bin"
    # Java
    "/usr/lib/jvm/java-17-openjdk/bin"
    # CUDA
    "/opt/cuda/bin"
    "/opt/cuda/nsight_compute"
    "/opt/cuda/nsight_systems/bin"
    # System paths (usually implicitly added, but good to be explicit)
    "/usr/local/sbin"
    "/usr/local/bin"
    "/usr/bin"
    "/usr/bin/site_perl"
    "/usr/bin/vendor_perl"
    "/usr/bin/core_perl"
)
export PATH

# 2. Environment Variables
export EDITOR=nano
export CHROME_EXECUTABLE="/usr/bin/google-chrome-stable"
export PICO_SDK_PATH="$HOME/pico-sdk"
export JAVA_HOME="/usr/lib/jvm/java-17-openjdk"
export LANG=en_US.UTF-8

# 3. History Configuration
# Keep a lot of history, but don't clutter it with duplicates.
HISTFILE=~/.zsh_history
HISTSIZE=10000
SAVEHIST=10000
setopt APPEND_HISTORY      # Append to history file, don't overwrite
setopt SHARE_HISTORY       # Share history between terminals
setopt HIST_IGNORE_DUPS    # Don't record duplicate commands
setopt HIST_IGNORE_SPACE   # Don't record commands starting with a space

# 4. Completion System
# Initialize the advanced completion system
autoload -Uz compinit
zstyle ':completion:*' menu select
zstyle ':completion:*' matcher-list 'm:{a-zA-Z}={A-Za-z}' # Case insensitive

# Smart cache check for Arch Linux (GNU stat)
# If .zcompdump exists and is less than 24 hours old, skip checks (-C)
_comp_dumpfile="${ZDOTDIR:-$HOME}/.zcompdump"
if [[ -f "$_comp_dumpfile" ]]; then
    # GNU stat uses '-c %Y' for modification time in seconds
    if [[ $(date +%s) -lt $(($(stat -c %Y "$_comp_dumpfile") + 86400)) ]]; then
        compinit -C
    else
        compinit
    fi
else
    compinit
fi

# 5. Keybindings
bindkey -e # Emacs mode (standard for Linux terminals)
bindkey '^[[3~' delete-char
bindkey '^[[1;5C' forward-word
bindkey '^[[1;5D' backward-word

# 6. Plugins (The Arch Linux Way)
# We source these directly from the system locations. 
# This is much faster than Oh-My-Zsh.

# Autosuggestions (Grey text preview)
if [ -f /usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh ]; then
    source /usr/share/zsh/plugins/zsh-autosuggestions/zsh-autosuggestions.zsh
    ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE=20
fi

# Syntax Highlighting (Must be sourced LAST)
if [ -f /usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh ]; then
    source /usr/share/zsh/plugins/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh
fi

# 7. Aliases
# Use 'bat' if available (Cat with wings)
if command -v bat &> /dev/null; then
    alias cat='bat --paging=never --style=plain'
    alias less='bat --paging=always'
fi

# Standard colors
alias grep='grep --color=auto'
alias ls='ls --color=auto'
alias ll='ls -lh'
alias la='ls -lha'

# Safety nets
alias cp='cp -i'
alias mv='mv -i'
alias rm='rm -i'

# Source your external aliases if the file exists
if [ -f "$HOME/.oh-my-zsh/custom/aliases.zsh" ]; then
    source "$HOME/.oh-my-zsh/custom/aliases.zsh"
fi

if [ -f "$HOME/.config/genoa/idle-inhibit.zsh" ]; then
    source "$HOME/.config/genoa/idle-inhibit.zsh"
fi

# 8. Starship Prompt
# Initialize the Starship prompt (replaces the theme)
if command -v starship &> /dev/null; then
    eval "$(starship init zsh)"
fi

# 9. One-Time Run (Fastfetch)
# Only run this if we are in an interactive shell (not a script)
if [[ -o interactive ]]; then
    if command -v fastfetch &> /dev/null; then
        fastfetch
    fi
fi
export PATH=$HOME/quick-start/rtems/7/bin:$PATH

if [ -f ~/.secrets ]; then
  source ~/.secrets
fi

