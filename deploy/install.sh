#!/usr/bin/env bash
# Board Game Tracker — installer for Raspberry Pi (or any Linux/systemd box).
#
# Usage (as root, e.g. via sudo):
#   curl -sSL https://raw.githubusercontent.com/siesta5787/board-game-tracker/master/deploy/install.sh | sudo bash
#
# Safe to re-run: it won't overwrite an existing .env or database, it just
# re-installs the binary/service (useful for re-running after a failure).

set -euo pipefail

REPO="siesta5787/board-game-tracker"
INSTALL_DIR="/opt/board-game-tracker"
SERVICE_USER="boardgame"

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run this as root (e.g. 'sudo bash install.sh')." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        echo "This installer supports 64-bit (aarch64) Raspberry Pi OS only." >&2
        echo "Make sure you flashed the 64-bit version of Raspberry Pi OS." >&2
        exit 1
        ;;
esac
echo "Detected architecture: $(uname -m) -> $TARGET"

echo "Installing prerequisites..."
apt-get update -qq
apt-get install -y -qq curl tar >/dev/null

if ! id "$SERVICE_USER" >/dev/null 2>&1; then
    echo "Creating service user '$SERVICE_USER'..."
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
fi

mkdir -p "$INSTALL_DIR"
TARBALL_URL="https://github.com/$REPO/releases/latest/download/board-game-tracker-$TARGET.tar.gz"
echo "Downloading latest release from $TARBALL_URL ..."
curl -sSL "$TARBALL_URL" -o /tmp/board-game-tracker.tar.gz
tar -xzf /tmp/board-game-tracker.tar.gz -C "$INSTALL_DIR"
rm /tmp/board-game-tracker.tar.gz
chmod +x "$INSTALL_DIR/board_game_tracker"

mkdir -p "$INSTALL_DIR/data/photos"

if [ ! -f "$INSTALL_DIR/.env" ]; then
    echo "No existing .env found — generating one with a fresh admin password."
    # `head -c 24` exiting early sends tr a SIGPIPE, which pipefail treats as
    # a pipeline failure and would abort the whole script under set -e — the
    # password itself is still captured correctly, so just swallow that.
    ADMIN_PASSWORD="$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24 || true)"
    cat >"$INSTALL_DIR/.env" <<EOF
DATABASE_URL=sqlite://data/boardgames.db
BIND_ADDR=127.0.0.1:3000
ADMIN_USERNAME=admin
ADMIN_PASSWORD=$ADMIN_PASSWORD
EOF
    PRINT_CREDENTIALS=1
else
    echo "Existing .env found — leaving it untouched."
    PRINT_CREDENTIALS=0
fi

chown -R "$SERVICE_USER:$SERVICE_USER" "$INSTALL_DIR"
chmod 600 "$INSTALL_DIR/.env"

echo "Installing systemd service..."
cat >/etc/systemd/system/board-game-tracker.service <<EOF
[Unit]
Description=Board Game Tracker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
WorkingDirectory=$INSTALL_DIR
EnvironmentFile=$INSTALL_DIR/.env
ExecStart=$INSTALL_DIR/board_game_tracker
Restart=on-failure
RestartSec=5

NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$INSTALL_DIR/data

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now board-game-tracker

echo "Installing the update watcher..."
# A separate, root-owned component that does the actual update/restart work.
# board_game_tracker itself runs unprivileged and can only ever drop a flag
# file in its own data/ folder asking for "update" or "restart" — it can
# never reach or modify anything in this directory, even if fully
# compromised, since it lives outside $INSTALL_DIR entirely and nothing here
# is boardgame-writable.
UPDATER_DIR="/opt/board-game-tracker-updater"
mkdir -p "$UPDATER_DIR"
cat >"$UPDATER_DIR/watcher.sh" <<'WATCHER_EOF'
#!/usr/bin/env bash
# Runs as root, triggered only when board-game-tracker (unprivileged) drops
# a flag file asking for an update or restart.
set -euo pipefail

FLAG_FILE="/opt/board-game-tracker/data/update_requested"
REPO="siesta5787/board-game-tracker"

ACTION="$(cat "$FLAG_FILE" 2>/dev/null || true)"
rm -f "$FLAG_FILE"

case "$ACTION" in
    update)
        curl -sSL "https://raw.githubusercontent.com/$REPO/master/deploy/update.sh" | bash
        ;;
    restart)
        systemctl restart board-game-tracker
        ;;
    *)
        echo "Unknown or empty update-request action: '$ACTION'" >&2
        exit 1
        ;;
esac
WATCHER_EOF
chown -R root:root "$UPDATER_DIR"
chmod 700 "$UPDATER_DIR"
chmod 700 "$UPDATER_DIR/watcher.sh"

cat >/etc/systemd/system/board-game-tracker-updater.path <<'PATH_EOF'
[Unit]
Description=Watch for Board Game Tracker update/restart requests

[Path]
PathExists=/opt/board-game-tracker/data/update_requested

[Install]
WantedBy=multi-user.target
PATH_EOF

cat >/etc/systemd/system/board-game-tracker-updater.service <<'SERVICE_EOF'
[Unit]
Description=Handle a pending Board Game Tracker update/restart request

[Service]
Type=oneshot
ExecStart=/opt/board-game-tracker-updater/watcher.sh
SERVICE_EOF

systemctl daemon-reload
systemctl enable --now board-game-tracker-updater.path

echo ""
echo "=========================================="
echo " Board Game Tracker is installed and running."
echo "=========================================="
echo ""
echo "Locally on the Pi: http://127.0.0.1:3000"
echo ""
if [ "$PRINT_CREDENTIALS" -eq 1 ]; then
    echo "First-time admin login:"
    echo "  Username: admin"
    echo "  Password: $ADMIN_PASSWORD"
    echo ""
    echo "Save this password now — it won't be shown again. You'll be forced"
    echo "to change it and set up two-factor login the first time you sign in."
    echo ""
fi
echo "This only listens on the Pi itself (127.0.0.1) for security. To reach it"
echo "from your phone or other devices, set up Tailscale Funnel next — see"
echo "DEPLOY.md for that step."
echo ""
echo "Future updates, backups, and restarts can all be done from the app's"
echo "Settings > Admin pages once logged in — you shouldn't need to SSH back"
echo "in for routine maintenance after this."
echo ""
echo "Check status any time with: systemctl status board-game-tracker"
