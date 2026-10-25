# Screenshots

The web interface of 0.1.0-beta.10, on a desktop browser (1440x900) and on a
phone (390x844). They were made with the built page and recorded, anonymized
answers of the test board's API: the addresses are documentation addresses
(RFC 5737, RFC 3849), the names are made up, and the target's screen is a
drawn stand-in, not a real screen.

## Remote desktop

![The remote desktop with the toolbar](screenshots/02-desktop.png)
The target's screen under the toolbar.

![Confirm remote control](screenshots/03-confirm-control.png)
The first click, tap or key on the screen asks before anything reaches the
target.

![Login](screenshots/01-login.png)
The login page.

## Screen menu

![Screen menu](screenshots/04-screen-menu.png)
Video mode, resolution, frame rate, scale, upscaling and frame detection;
the H.264 encoder in use.

![Video mode](screenshots/05-screen-video-mode.png)
H.264 on the video engine or with x264, VP8 over WebRTC, or MJPEG.

![Resolution](screenshots/06-screen-resolution.png)
![Frame rate](screenshots/06b-screen-fps.png)
![Upscaling](screenshots/07-screen-upscaling.png)
Upscaling with its sharpness and size (this headless browser has no
graphics processor, which the menu says).

## Mouse, keyboard and monitors

![Mouse menu](screenshots/08-mouse-menu.png)
Cursor style, mouse mode, the desktop layout, pointer and wheel settings,
and Release remote control.

![Desktop layout set by hand](screenshots/09-mouse-desktop.png)
The desktop layout set by hand: the whole desktop, or this screen's place
in a desktop of several monitors.

![Desktop layout from the target agent](screenshots/09b-mouse-desktop-agent.png)
With the target agent's monitors read, the layout is theirs: duplicated
monitors are the whole desktop.

![Keyboard menu](screenshots/10-keyboard-menu.png)
Paste, the on-screen keyboard, shortcuts, and with the target agent the
target's layout, clipboard and lock.

![Monitors menu](screenshots/11-monitors-menu.png)
The target agent's monitors: the mode they are in, Windows' choices, and
which one is the board's.

## Media, power and the quality monitor

![Virtual media](screenshots/12-virtual-media.png)
Images to mount as a mass storage device or a CD-ROM.

![Image download](screenshots/12b-download-image.png)
![Wake-on-LAN](screenshots/12c-wake-on-lan.png)
![Power](screenshots/13-power-menu.png)
The ATX buttons: reset, a short and a long press of power.

![Quality monitor](screenshots/14-quality-monitor.png)
The quality monitor's Extended level.

## Settings

![About](screenshots/20-settings-about.png)
![Diagnostics](screenshots/21-settings-diagnostics.png)
![Appearance](screenshots/22-settings-appearance.png)
![Device](screenshots/23-settings-device.png)
![Device, continued](screenshots/24-settings-device-end.png)
Device: SSH, mDNS, HID-only mode, the target agent, the board's display,
the mouse jiggler, the session lock, reboot.

![System: network](screenshots/25-settings-system.png)
![System: network, continued](screenshots/26-settings-system-end.png)
System > Network: HTTPS, the transport (QUIC on the board, and this
browser's choice), Wi-Fi, IPv4, DNS, IPv6, TLS.

![System: time](screenshots/27-settings-system-time.png)
![System: firewall](screenshots/28-settings-system-firewall.png)
The firewall's four modes; a change rolls back by itself unless it is
confirmed.

![System: system log](screenshots/29-settings-system-system-log.png)
![Tailscale](screenshots/30-settings-tailscale.png)
![Updates](screenshots/31-settings-update.png)
![Updates, continued](screenshots/32-settings-update-end.png)
Signed application slots and signed RAW system updates, each verified on
the board.

![Account](screenshots/33-settings-account.png)

## Phone

![Phone, portrait](screenshots/40-phone.png)
![Phone, landscape](screenshots/41-phone-landscape.png)
![Phone with the extra keys](screenshots/42-phone-keyboard-key-row.png)
![Phone settings](screenshots/43-phone-settings.png)
The touch layout: the screen as a trackpad, the row of extra keys, and the
settings on the whole screen.
