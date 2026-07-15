# Deploying to a Raspberry Pi

These steps get Board Game Tracker running on a Raspberry Pi (tested target: Pi Zero 2 W, 64-bit). Only 64-bit Raspberry Pi OS is supported — the installer will error out clearly on a 32-bit install.

## 1. Flash the SD card

1. Download [Raspberry Pi Imager](https://www.raspberrypi.com/software/) on your computer.
2. Pick your Pi model, then choose **Raspberry Pi OS Lite (64-bit)** as the OS.
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

**First, update the OS itself** before installing anything else:

```
sudo apt update && sudo apt full-upgrade -y
sudo reboot
```

A freshly-flashed image has never had a single package update applied, so this is often a large download and a slow first run (dozens of minutes on a Pi Zero 2 W) — much better to get it out of the way now, on a Pi with nothing else running yet, than to hit it later from the app's System updates page while you're actively using it.

Once it's rebooted and reconnected, run the installer:

```
curl -sSL https://raw.githubusercontent.com/siesta5787/board-game-tracker/master/deploy/install.sh | sudo bash
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

Once installed, updates normally don't need SSH at all — log in as an admin and go to **Settings > Admin > Software update**, which checks GitHub for a newer release and updates + restarts the app with one click.

The one exception: if a release adds new system-level components (a new systemd unit, for example), you'll need to re-run the installer once over SSH to pick that up:

```
curl -sSL https://raw.githubusercontent.com/siesta5787/board-game-tracker/master/deploy/install.sh | sudo bash
```

It's safe to re-run any time — it never touches your existing `.env` or database.

## System updates (OS + Tailscale)

**Settings > Admin > System updates** shows pending Raspberry Pi OS package updates and Tailscale's version, with buttons to install updates, update Tailscale, or reboot (if a reboot is needed) — no SSH required. You can also set a schedule (daily/weekly/monthly, at a chosen time) to check automatically, and optionally auto-install and auto-reboot.

## Useful commands on the Pi

- Check it's running: `systemctl status board-game-tracker`
- View logs: `journalctl -u board-game-tracker -f`
- Restart it: `sudo systemctl restart board-game-tracker`

## Backups

**Settings > Admin > Backups** has two independent layers of protection:

- **Named snapshots**: create, download, and delete backups (a full database snapshot plus all play photos, zipped) on demand or on a schedule (daily/weekly/monthly), with automatic pruning of old ones. Download a copy somewhere off the Pi's SD card every so often — SD cards are the least reliable part of a Pi setup. The page always shows a plain-language summary of whatever schedule is currently active (e.g. "Backing up daily at 02:00, keeping the last 14"), so you don't have to remember what you configured.
- **Continuous mirror**: a separate toggle that keeps a live, always-current copy of the database + photos refreshed every 1-2 minutes. Much tighter recovery window than the scheduled snapshots, at the cost of only ever having "the latest" rather than named points in time — the two are meant to complement each other, not replace one another.

Both can copy to the same external USB drive (into separate `snapshots/` and `live/` folders), protecting against SD card failure, not just accidental data loss.

### Restoring a backup

Next to any backup in the list, expand **Restore…** and type the exact filename shown to confirm — this stops the app, replaces the live database and photos with that backup's contents, and restarts, no SSH required. It always keeps a raw safety copy of whatever was live immediately beforehand (in `data/backups/prerestore-<timestamp>/`), in case the wrong backup gets picked.

If you have a backup that isn't in the list — downloaded earlier, or recovered from the external drive after losing the Pi's own copy — expand **Upload a backup from elsewhere** above the list, choose the .zip file, and it'll be added to the list (and can then be restored the same way as any other).

### Setting up an external drive

Plug a USB drive into the Pi, go to **Settings > Admin > Backups**, and use the **Format drive** button in the "External drive" section — it only ever lists drives the kernel reports as removable (never the Pi's own SD card), shows the exact device and size, and requires typing the drive's size to confirm before doing anything, since formatting is irreversible. It formats as ext4, mounts it at `/mnt/board-game-backup`, and adds it to `/etc/fstab` so it survives reboots — no SSH needed.

If you'd rather do this manually over SSH instead (e.g. the drive isn't showing up as removable), the mount point the app expects is `/mnt/board-game-backup`:

```
lsblk                                  # find your drive, e.g. sda
sudo mkfs.ext4 /dev/sda                # erases the drive
sudo blkid /dev/sda                    # note the UUID
sudo mkdir -p /mnt/board-game-backup
```

Add a line to `/etc/fstab`:
```
UUID=xxxx-xxxx-xxxx  /mnt/board-game-backup  ext4  defaults,nofail  0  2
```

Then `sudo mount -a`. Either way, once it's mounted at that path, enable "copy backups to the external drive" and/or "continuously mirror" in the app — the page shows whether the drive is currently detected as connected, and no further SSH is needed after this one-time setup.
