#!/usr/bin/env bash
# Board Game Tracker — installer for Raspberry Pi (or any Linux/systemd box).
#
# Usage (as root, e.g. via sudo):
#   curl -sSL https://raw.githubusercontent.com/REPO_OWNER/REPO_NAME/main/deploy/install.sh | sudo bash
#
# Safe to re-run: it won't overwrite an existing .env or database, it just
# re-installs the binary/service (useful for re-running after a failure).

set -euo pipefail

REPO="REPO_OWNER/REPO_NAME"
INSTALL_DIR="/opt/board-game-tracker"
SERVICE_USER="boardgame"

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run this as root (e.g. 'sudo bash install.sh')." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    armv7l) TARGET="armv7-unknown-linux-musleabihf" ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        echo "This installer supports 64-bit (aarch64) and 32-bit ARMv7 Raspberry Pi OS only." >&2
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
    ADMIN_PASSWORD="$(tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24)"
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
echo "Check status any time with: systemctl status board-game-tracker"
