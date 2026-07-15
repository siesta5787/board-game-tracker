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

# "latest/download/..." redirects to "download/vX.Y.Z/...", which is the only
# place the actual version tag shows up in this whole download flow. Recorded
# so the app can tell you when the root-side watcher/scheduler scripts (only
# ever refreshed by re-running this installer, never by the in-app update
# button) have fallen behind the app version, instead of silently no-op'ing
# on features the installed watcher doesn't know about yet.
INSTALLED_VERSION="$(curl -sI "$TARBALL_URL" | grep -i '^location:' | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"

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

if [ -n "$INSTALLED_VERSION" ]; then
    echo -n "$INSTALLED_VERSION" >"$INSTALL_DIR/data/watcher_version"
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
systemctl enable board-game-tracker
# `enable --now` only *starts* the unit, which is a no-op if it's already
# running — meaning re-running this installer to pick up a new release
# would silently keep the old binary running forever. Restart explicitly so
# this works the same whether this is a fresh install or a re-run.
systemctl restart board-game-tracker

echo "Installing the update watcher and scheduler..."
# A separate, root-owned component that does the actual privileged work —
# updating the app itself, apt packages, Tailscale, and rebooting.
# board_game_tracker itself runs unprivileged and can only ever drop a flag
# file in its own data/ folder asking for one of a small fixed set of
# actions — it can never reach or modify anything in this directory, even if
# fully compromised, since it lives outside $INSTALL_DIR entirely and
# nothing here is boardgame-writable.
UPDATER_DIR="/opt/board-game-tracker-updater"
mkdir -p "$UPDATER_DIR"

# Placeholder mount point for an optional external backup drive. Doesn't do
# anything by itself — mount a drive here (e.g. via /etc/fstab) and enable
# "copy backups to external drive" in Settings > Admin > Backups to use it.
mkdir -p /mnt/board-game-backup

# Shared privileged actions, used both by the manual (flag-file-triggered)
# watcher and the automatic (time-triggered) scheduler, so the actual
# commands only need to be reviewed/maintained in one place.
cat >"$UPDATER_DIR/actions.sh" <<'ACTIONS_EOF'
#!/usr/bin/env bash
set -euo pipefail

REPO="siesta5787/board-game-tracker"

action_app_update() {
    curl -sSL "https://raw.githubusercontent.com/$REPO/master/deploy/update.sh" | bash
}

action_app_restart() {
    systemctl restart board-game-tracker
}

action_os_check() {
    apt-get update -qq
}

action_os_upgrade() {
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get upgrade -y -qq
}

action_tailscale_update() {
    tailscale update --yes
}

action_reboot() {
    systemctl reboot
}

# Formats a removable drive as ext4 and mounts it at the external backup
# path. Defense in depth: independently re-validates the device from
# scratch — matches the expected /dev/sd[a-z] pattern, is actually marked
# removable by the kernel, and is definitely not whatever backs / or /boot
# — regardless of what the (unprivileged, internet-facing) app already
# checked before requesting this, since that process is exactly the one
# component that must never be trusted to have gotten this right on its own.
action_format_drive() {
    local device="$1"

    if [ -z "$device" ]; then
        echo "format_drive: no device specified" >&2
        return 1
    fi
    if ! echo "$device" | grep -qE '^/dev/sd[a-z]$'; then
        echo "format_drive: refusing to format '$device' — doesn't match the expected /dev/sd[a-z] pattern" >&2
        return 1
    fi

    local devname
    devname="$(basename "$device")"
    if [ ! -f "/sys/block/$devname/removable" ] || [ "$(cat "/sys/block/$devname/removable")" != "1" ]; then
        echo "format_drive: refusing to format '$device' — not marked removable by the kernel" >&2
        return 1
    fi

    local root_device boot_device
    root_device="$(findmnt -no SOURCE / | sed -E 's/p?[0-9]+$//')"
    if [ "$device" = "$root_device" ]; then
        echo "format_drive: refusing to format '$device' — this is the root filesystem's device" >&2
        return 1
    fi
    boot_device="$(findmnt -no SOURCE /boot 2>/dev/null | sed -E 's/p?[0-9]+$//' || true)"
    if [ -n "$boot_device" ] && [ "$device" = "$boot_device" ]; then
        echo "format_drive: refusing to format '$device' — this is the boot device" >&2
        return 1
    fi

    echo "Formatting $device as ext4..."
    umount "${device}"* 2>/dev/null || true
    mkfs.ext4 -F -q "$device"

    mkdir -p /mnt/board-game-backup
    local uuid
    uuid="$(blkid -s UUID -o value "$device")"

    # Replace any existing fstab entry for this mount point first, so
    # re-formatting a previously-configured drive doesn't leave stale
    # duplicate entries behind.
    sed -i '\#/mnt/board-game-backup#d' /etc/fstab
    echo "UUID=$uuid /mnt/board-game-backup ext4 defaults,nofail 0 2" >>/etc/fstab

    mount /mnt/board-game-backup
    echo "Drive formatted and mounted at /mnt/board-game-backup."
}
ACTIONS_EOF

cat >"$UPDATER_DIR/watcher.sh" <<'WATCHER_EOF'
#!/usr/bin/env bash
# Runs as root, triggered only when board-game-tracker (unprivileged) drops
# a flag file asking for one of a fixed set of actions.
set -euo pipefail
source /opt/board-game-tracker-updater/actions.sh

FLAG_FILE="/opt/board-game-tracker/data/update_requested"
FLAG_CONTENT="$(cat "$FLAG_FILE" 2>/dev/null || true)"
rm -f "$FLAG_FILE"

# Most actions are a single word; format_drive additionally carries a
# device path as a second, space-separated token.
ACTION="${FLAG_CONTENT%% *}"
if [ "$ACTION" != "$FLAG_CONTENT" ]; then
    ARG="${FLAG_CONTENT#* }"
else
    ARG=""
fi

case "$ACTION" in
    update) action_app_update ;;
    restart) action_app_restart ;;
    os_check) action_os_check ;;
    os_upgrade) action_os_upgrade ;;
    tailscale_update) action_tailscale_update ;;
    reboot) action_reboot ;;
    format_drive) action_format_drive "$ARG" ;;
    *)
        echo "Unknown or empty update-request action: '$ACTION'" >&2
        exit 1
        ;;
esac
WATCHER_EOF

# Runs on a timer (not flag-triggered) to apply the admin-configured
# schedule from Settings > Admin > System updates. This script is already
# root, so unlike the manual path above it just acts directly rather than
# going through the flag-file indirection — that indirection exists only to
# let the *unprivileged, internet-facing* app request privileged actions,
# which doesn't apply to this trusted, non-network-facing component.
cat >"$UPDATER_DIR/scheduler.sh" <<'SCHEDULER_EOF'
#!/usr/bin/env bash
set -euo pipefail
source /opt/board-game-tracker-updater/actions.sh

CONFIG_FILE="/opt/board-game-tracker/data/schedule.conf"
LAST_RUN_FILE="/opt/board-game-tracker-updater/last_run_date"

[ -f "$CONFIG_FILE" ] || exit 0

FREQUENCY="daily"
DAY_OF_WEEK="0"
DAY_OF_MONTH="1"
CHECK_TIME="03:00"
AUTO_APPLY_OS="false"
AUTO_APPLY_TAILSCALE="false"
AUTO_REBOOT="false"
# shellcheck disable=SC1090
source "$CONFIG_FILE"

TODAY="$(date +%F)"
LAST_RUN="$(cat "$LAST_RUN_FILE" 2>/dev/null || true)"
[ "$LAST_RUN" != "$TODAY" ] || exit 0

case "$FREQUENCY" in
    weekly)
        [ "$(date +%w)" = "$DAY_OF_WEEK" ] || exit 0
        ;;
    monthly)
        [ "$(date +%-d)" = "$DAY_OF_MONTH" ] || exit 0
        ;;
esac

NOW_MINUTES=$(( 10#$(date +%H) * 60 + 10#$(date +%M) ))
TARGET_MINUTES=$(( 10#${CHECK_TIME%%:*} * 60 + 10#${CHECK_TIME##*:} ))
DIFF=$(( NOW_MINUTES - TARGET_MINUTES ))
# Only fire in the 10-minute window right after the scheduled time (this
# runs on a 10-minute timer, so this is the precision that's actually
# achievable — good enough for background maintenance).
[ "$DIFF" -ge 0 ] && [ "$DIFF" -lt 10 ] || exit 0

echo "$TODAY" >"$LAST_RUN_FILE"

action_os_check
if [ "$AUTO_APPLY_OS" = "true" ]; then
    action_os_upgrade
fi
if [ "$AUTO_APPLY_TAILSCALE" = "true" ]; then
    action_tailscale_update
fi
if [ "$AUTO_REBOOT" = "true" ] && [ -f /var/run/reboot-required ]; then
    action_reboot
fi
SCHEDULER_EOF

# Mirrors backups (both the named point-in-time snapshots and the
# continuously-refreshed live-mirror file) to the external drive, if
# enabled and the drive is currently mounted. Runs on its own much faster
# timer than the OS-update scheduler above, since a stale offsite copy
# defeats the point of having one. Reads just the settings it needs via
# grep rather than sourcing backup_schedule.conf, since that file and
# schedule.conf (above) share variable names (FREQUENCY, DAY_OF_WEEK, ...)
# and sourcing both in the same script would let one silently clobber the
# other.
cat >"$UPDATER_DIR/backup_sync.sh" <<'BACKUP_SYNC_EOF'
#!/usr/bin/env bash
set -euo pipefail

BACKUP_CONFIG_FILE="/opt/board-game-tracker/data/backup_schedule.conf"
EXTERNAL_MOUNT="/mnt/board-game-backup"
DATA_DIR="/opt/board-game-tracker/data"

[ -f "$BACKUP_CONFIG_FILE" ] || exit 0
mountpoint -q "$EXTERNAL_MOUNT" || exit 0

EXTERNAL_COPY_ENABLED="$(grep -m1 '^EXTERNAL_COPY_ENABLED=' "$BACKUP_CONFIG_FILE" | cut -d= -f2 || true)"
CONTINUOUS_MIRROR_ENABLED="$(grep -m1 '^CONTINUOUS_MIRROR_ENABLED=' "$BACKUP_CONFIG_FILE" | cut -d= -f2 || true)"

if [ "$EXTERNAL_COPY_ENABLED" = "true" ] && [ -d "$DATA_DIR/backups" ]; then
    mkdir -p "$EXTERNAL_MOUNT/snapshots"
    cp -au "$DATA_DIR/backups/." "$EXTERNAL_MOUNT/snapshots/" 2>/dev/null || true
fi

if [ "$CONTINUOUS_MIRROR_ENABLED" = "true" ] && [ -f "$DATA_DIR/live_mirror.db" ]; then
    mkdir -p "$EXTERNAL_MOUNT/live"
    cp -au "$DATA_DIR/live_mirror.db" "$EXTERNAL_MOUNT/live/boardgames.db" 2>/dev/null || true
    if [ -d "$DATA_DIR/photos" ]; then
        mkdir -p "$EXTERNAL_MOUNT/live/photos"
        cp -au "$DATA_DIR/photos/." "$EXTERNAL_MOUNT/live/photos/" 2>/dev/null || true
    fi
fi
BACKUP_SYNC_EOF

chown -R root:root "$UPDATER_DIR"
chmod 700 "$UPDATER_DIR" "$UPDATER_DIR/actions.sh" "$UPDATER_DIR/watcher.sh" "$UPDATER_DIR/scheduler.sh" "$UPDATER_DIR/backup_sync.sh"

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

cat >/etc/systemd/system/board-game-tracker-scheduler.timer <<'TIMER_EOF'
[Unit]
Description=Check the Board Game Tracker update/reboot schedule every 10 minutes

[Timer]
OnBootSec=2min
OnUnitActiveSec=10min

[Install]
WantedBy=timers.target
TIMER_EOF

cat >/etc/systemd/system/board-game-tracker-scheduler.service <<'SCHED_SERVICE_EOF'
[Unit]
Description=Apply the Board Game Tracker scheduled update/reboot, if due

[Service]
Type=oneshot
ExecStart=/opt/board-game-tracker-updater/scheduler.sh
SCHED_SERVICE_EOF

cat >/etc/systemd/system/board-game-tracker-backup-sync.timer <<'BACKUP_TIMER_EOF'
[Unit]
Description=Sync Board Game Tracker backups to the external drive every 90 seconds

[Timer]
OnBootSec=1min
OnUnitActiveSec=90sec

[Install]
WantedBy=timers.target
BACKUP_TIMER_EOF

cat >/etc/systemd/system/board-game-tracker-backup-sync.service <<'BACKUP_SERVICE_EOF'
[Unit]
Description=Mirror Board Game Tracker backups to the external drive, if enabled

[Service]
Type=oneshot
ExecStart=/opt/board-game-tracker-updater/backup_sync.sh
BACKUP_SERVICE_EOF

systemctl daemon-reload
systemctl enable --now board-game-tracker-updater.path
systemctl enable --now board-game-tracker-scheduler.timer
systemctl enable --now board-game-tracker-backup-sync.timer

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
