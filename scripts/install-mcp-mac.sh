#!/usr/bin/env bash
# Build clingate's tools for Claude and add them to Claude Desktop, on a Mac.
#
# Needs the Xcode command line tools and Rust already installed. Does the rest:
# checks git can reach the private repositories (and signs in to GitHub with
# `gh` if it cannot), clones or updates clingate, builds the server, copies it
# to ~/bin, adds it to Claude Desktop's config (backing the old one up), and
# checks the program answers as Claude Desktop will ask it.
#
# Run it from anywhere:
#
#     bash install-mcp-mac.sh
#
# Run it again to update. Settings, if the defaults do not suit:
#
#     CLINGATE_DIR     where the code is kept    (default ~/clingate)
#     CLINGATE_BRANCH  the branch to build       (default below)
#     CLINGATE_BIN     where the program goes    (default ~/bin)

set -euo pipefail

# The server is on this branch until it is merged; then this becomes `main`.
BRANCH="${CLINGATE_BRANCH:-claude/funny-bardeen-bleqcn}"
DIR="${CLINGATE_DIR:-$HOME/clingate}"
BIN="${CLINGATE_BIN:-$HOME/bin}"
REPO="https://github.com/czarop/clingate"
FLOW="https://github.com/czarop/flow"
CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[31mStopped: %s\033[0m\n' "$*" >&2; exit 1; }

# ── what has to be here already ──────────────────────────────────────────
step "Checking the tools this needs"
command -v git >/dev/null || fail "git is missing - run: xcode-select --install"
command -v python3 >/dev/null || fail "python3 is missing - run: xcode-select --install"
if ! command -v cargo >/dev/null; then
    # A new shell reads this; this one may have been opened before Rust was installed.
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi
command -v cargo >/dev/null || fail "Rust is missing - install it from https://rustup.rs"
echo "git $(git --version | cut -d' ' -f3), $(cargo --version)"

# ── GitHub: both repositories are private ────────────────────────────────
step "Checking git can reach the private repositories"
can_reach() { GIT_TERMINAL_PROMPT=0 git ls-remote "$1" >/dev/null 2>&1; }
if ! can_reach "$FLOW"; then
    echo "git cannot reach $FLOW yet. Signing in to GitHub with gh."
    if ! command -v gh >/dev/null; then
        command -v brew >/dev/null || fail "gh is not installed and neither is Homebrew. Install gh from https://cli.github.com, then run this again."
        brew install gh
    fi
    gh auth status --hostname github.com >/dev/null 2>&1 \
        || gh auth login --hostname github.com --git-protocol https --web
    # Let git use gh's sign-in for https://github.com.
    gh auth setup-git --hostname github.com
    can_reach "$FLOW" || fail "git still cannot reach $FLOW. Check your GitHub account can see czarop/flow."
fi
can_reach "$REPO" || fail "git cannot reach $REPO. Check your GitHub account can see czarop/clingate."
echo "Both repositories are reachable."

# ── the code ─────────────────────────────────────────────────────────────
if [ -d "$DIR/.git" ]; then
    step "Updating $DIR to $BRANCH"
    if [ -n "$(git -C "$DIR" status --porcelain --untracked-files=no)" ]; then
        fail "$DIR has changes of your own. Commit or stash them, then run this again."
    fi
    git -C "$DIR" fetch origin "$BRANCH"
    git -C "$DIR" checkout "$BRANCH" 2>/dev/null || git -C "$DIR" checkout -b "$BRANCH" "origin/$BRANCH"
    git -C "$DIR" merge --ff-only "origin/$BRANCH"
else
    [ -e "$DIR" ] && fail "$DIR exists but is not a clone of clingate. Move it, or set CLINGATE_DIR."
    step "Cloning clingate ($BRANCH) into $DIR"
    git clone --branch "$BRANCH" "$REPO" "$DIR"
fi

# ── build ────────────────────────────────────────────────────────────────
step "Building the server (the first build takes several minutes)"
( cd "$DIR" && cargo build --release -p clingate-mcp )

step "Installing it in $BIN"
mkdir -p "$BIN"
# Copied beside it, then renamed over it: a copy of Claude Desktop still
# running the old one keeps running it rather than having it change under it.
cp "$DIR/target/release/clingate-mcp" "$BIN/.clingate-mcp.new"
mv -f "$BIN/.clingate-mcp.new" "$BIN/clingate-mcp"
PROGRAM="$BIN/clingate-mcp"
echo "$PROGRAM"

# ── check it answers ─────────────────────────────────────────────────────
step "Checking the server answers as Claude Desktop will ask it"
python3 - "$PROGRAM" <<'PY'
import json, subprocess, sys
p = subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True)
def send(m):
    p.stdin.write(json.dumps(m) + "\n"); p.stdin.flush()
try:
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "install-check", "version": "0"}}})
    init = json.loads(p.stdout.readline())
    assert init["result"]["serverInfo"]["name"] == "clingate", init
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
    tools = json.loads(p.stdout.readline())["result"]["tools"]
    print(f"It answers, with {len(tools)} tools.")
except Exception as e:
    sys.exit(f"The server did not answer properly: {e}")
finally:
    p.kill()
PY

# ── Claude Desktop's config ──────────────────────────────────────────────
step "Adding it to Claude Desktop"
mkdir -p "$(dirname "$CONFIG")"
if [ -f "$CONFIG" ]; then
    BACKUP="$CONFIG.backup-$(date +%Y%m%d-%H%M%S)"
    cp "$CONFIG" "$BACKUP"
    echo "The old config is kept as $BACKUP"
fi
python3 - "$CONFIG" "$PROGRAM" <<'PY'
import json, os, sys
path, program = sys.argv[1], sys.argv[2]
config = {}
if os.path.exists(path) and os.path.getsize(path) > 0:
    try:
        with open(path) as f:
            config = json.load(f)
    except ValueError as e:
        sys.exit(f"{path} is not valid JSON ({e}); fix it by hand, then run this again")
servers = config.setdefault("mcpServers", {})
entry = servers.get("clingate", {})
entry["command"] = program  # anything else there - an env block - is kept
servers["clingate"] = entry
with open(path, "w") as f:
    json.dump(config, f, indent=2)
    f.write("\n")
print(f"{path}: clingate -> {program}")
PY

# ── restart ──────────────────────────────────────────────────────────────
step "Done"
if pgrep -xq Claude; then
    read -r -p "Claude Desktop is running and needs a restart to pick this up. Restart it now? [y/N] " answer
    if [[ "$answer" =~ ^[Yy] ]]; then
        osascript -e 'quit app "Claude"'
        while pgrep -xq Claude; do sleep 1; done
        open -a Claude
        echo "Restarted."
    else
        echo "Quit Claude Desktop (Cmd-Q) and open it again when you are ready."
    fi
else
    echo "Open Claude Desktop: the clingate tools are under the tools menu in a new chat."
fi
cat <<EOF

If the tools do not appear, Claude Desktop's log says why:
    ~/Library/Logs/Claude/mcp-server-clingate.log
If opening a workspace fails with a permission error, allow Claude in
System Settings > Privacy & Security > Files and Folders.
EOF
