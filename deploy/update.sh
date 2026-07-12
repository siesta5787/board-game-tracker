#!/usr/bin/env bash
# Board Game Tracker — updater. Downloads the latest release and replaces
# the running app, without touching your .env or your data (database +
# photos). Database migrations run automatically on the next startup.
#
# Usage (as root, e.g. via sudo):
#   curl -sSL https://raw.githubusercontent.com/REPO_OWNER/REPO_NAME/main/deploy/update.sh | sudo bash

set -euo pipefail

REPO="REPO_OWNER/REPO_NAME"
INSTALL_DIR="/opt/board-game-tracker"
SERVICE_USER="boardgame"

if [ "$(id -u)" -ne 0 ]; then
    echo "Please run this as root (e.g. 'sudo bash update.sh')." >&2
    exit 1
fi

if [ ! -f "$INSTALL_DIR/.env" ]; then
    echo "$INSTALL_DIR doesn't look like an existing install (no .env found)." >&2
    echo "Run install.sh first." >&2
    exit 1
fi

case "$(uname -m)" in
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    armv7l) TARGET="armv7-unknown-linux-musleabihf" ;;
    *)
        echo "Unsupported architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

TARBALL_URL="https://github.com/$REPO/releases/latest/download/board-game-tracker-$TARGET.tar.gz"
echo "Downloading latest release from $TARBALL_URL ..."
curl -sSL "$TARBALL_URL" -o /tmp/board-game-tracker.tar.gz

echo "Stopping service..."
systemctl stop board-game-tracker

echo "Installing update..."
TMP_EXTRACT="$(mktemp -d)"
tar -xzf /tmp/board-game-tracker.tar.gz -C "$TMP_EXTRACT"
rm /tmp/board-game-tracker.tar.gz

cp "$TMP_EXTRACT/board_game_tracker" "$INSTALL_DIR/board_game_tracker"
chmod +x "$INSTALL_DIR/board_game_tracker"

rm -rf "$INSTALL_DIR/static"
cp -r "$TMP_EXTRACT/static" "$INSTALL_DIR/static"
rm -rf "$TMP_EXTRACT"

chown -R "$SERVICE_USER:$SERVICE_USER" "$INSTALL_DIR/board_game_tracker" "$INSTALL_DIR/static"

echo "Starting service..."
systemctl start board-game-tracker

sleep 2
if systemctl is-active --quiet board-game-tracker; then
    echo ""
    echo "Update complete and running."
else
    echo ""
    echo "The service didn't start cleanly — check 'systemctl status board-game-tracker'" >&2
    echo "and 'journalctl -u board-game-tracker -n 50' for details." >&2
    exit 1
fi
