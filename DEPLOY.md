# Deploying to a Raspberry Pi

These steps get Board Game Tracker running on a Raspberry Pi (tested target: Pi Zero 2 W, 64-bit — but works on any Pi that can run 64-bit or 32-bit Raspberry Pi OS).

## 1. Flash the SD card

1. Download [Raspberry Pi Imager](https://www.raspberrypi.com/software/) on your computer.
2. Pick your Pi model, then choose **Raspberry Pi OS Lite (64-bit)** as the OS (or the 32-bit version if your Pi is older/32-bit only).
3. Click the gear icon (⚙) before writing — this is the important part. Set:
   - A hostname (e.g. `boardgames`)
   - Enable SSH, with a username and password (or your SSH key)
   - Your WiFi network name and password, if not using Ethernet
4. Write the image, then put the SD card in the Pi and power it on. Give it a minute or two to boot.

## 2. Connect and install

From your computer, SSH into the Pi (replace with the hostname or IP you set):

```
ssh username@boardgames.local
```

Then run the installer:

```
curl -sSL https://raw.githubusercontent.com/REPO_OWNER/REPO_NAME/main/deploy/install.sh | sudo bash
```

This downloads the right binary for your Pi automatically, sets it up as a background service that starts on boot, and prints an admin username/password at the end — **save that password**, you'll need it to log in the first time (and you'll be asked to change it and set up two-factor login immediately after).

The app is now running, but only reachable from the Pi itself (`127.0.0.1:3000`) — that's intentional for security. The next step makes it reachable from your phone and your friends' devices.

## 3. Make it reachable: Tailscale Funnel

[Tailscale](https://tailscale.com/) gives you a real public HTTPS URL without opening any ports on your router.

1. Install Tailscale on the Pi:
   ```
   curl -fsSL https://tailscale.com/install.sh | sh
   sudo tailscale up
   ```
   This prints a login link — open it on your phone or computer and sign in (a free personal Tailscale account is enough).
2. Turn on Funnel for the app's port:
   ```
   sudo tailscale funnel 3000
   ```
3. Tailscale will print your public URL (something like `https://boardgames.your-tailnet.ts.net`). That's the address to give your friends.

## Updating later

Whenever a new version is released, SSH into the Pi and run:

```
curl -sSL https://raw.githubusercontent.com/REPO_OWNER/REPO_NAME/main/deploy/update.sh | sudo bash
```

This replaces the app with the latest version and restarts it. Your database, collection, plays, and photos are never touched.

## Useful commands on the Pi

- Check it's running: `systemctl status board-game-tracker`
- View logs: `journalctl -u board-game-tracker -f`
- Restart it: `sudo systemctl restart board-game-tracker`

## Backups

Everything the app stores lives in `/opt/board-game-tracker/data/` (the SQLite database and play photos). Back up that one folder and you have everything. A simple approach: a cron job that copies it somewhere off the SD card periodically — SD cards are the least reliable part of a Pi setup.
