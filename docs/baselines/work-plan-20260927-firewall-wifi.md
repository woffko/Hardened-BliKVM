# Work plan: firewall page and Wi-Fi (2026-09-27)

Follows `work-plan-20260926.md`. Rules as there: default-off or
capability-gated surfaces enforced by the server and the board owner,
privileged work only in the owner or a dedicated helper, English repository
text, secrets only through `project-memory-exec` (never in arguments, logs or
receipts), `scripts/check.sh` green before every commit, every change shipped
as a signed slot (and update runtime package when units or helpers change)
installed through the API and verified on the test board, audit and design
documents updated with evidence under `evidence/private`.

## Starting state

- Slot 43: the firewall section renders the reference page on the reference
  contract (`/api/system/firewall`, `/api/system/firewall/confirm`), with
  the owner's four-mode runtime, 60-second rollback and restore after reboot
  behind it. Board configuration blocks WebRTC in Restricted and Paranoid.
- Found on the board: applying Restricted from the page returned 503 although
  the owner applied it. The following status read (about nine `nft` runs
  through transient systemd units, three of them the same
  `nft list ruleset`) exceeded the server's 5-second control timeout, so the
  page never confirmed and the owner rolled the change back. The owner also
  serves the firewall state inside its shared lock, so HID requests wait
  while the page or the video menu reads the firewall.
- Wi-Fi: `wlan0` (RTL8723DS) works and scans. The section offers only a
  typed SSID and password; connecting cannot succeed (the profile is brought
  up by its file name, not its `id`, and never loaded), only WPA-PSK is
  written, and only a profile connected in the same owner process can be
  disconnected. There is no network list, saved networks or radio switch.
  The reference has no Wi-Fi section; this one is ours.

## Phase F: firewall page in working order

F1. Owner: one `nft list ruleset` snapshot, reused for 10 s, serves the rule
    preview and the IPv4, IPv6 and nft listings; the state view reads the
    ruleset once instead of three times.
F2. Owner: the firewall state, baseline and rule requests run outside the
    shared owner state (their own lock, a blocking worker), like the mode
    runtime, so HID and media never wait on `nft`.
F3. Server: firewall calls use a control client with a 30-second timeout; a
    status read that fails right after an applied change is retried once;
    the video menu's status read stays cheap (short server-side cache).
F4. Board: through the page, desktop and mobile, apply each mode; Restricted
    and Paranoid confirm themselves; an unconfirmed change (API only) rolls
    back after 60 s; the confirmed mode returns after a reboot; the video
    menu offers only MJPEG in Restricted and Paranoid; HID latency is
    measured while the page loads. End in `baseline`.
F5. Audit, design document, release, `check.sh`.

## Phase W: Wi-Fi section

W1. Protocol: `WifiScan {rescan}`, `WifiSaved {}`, `WifiProfile {id, action}`
    (connect, forget, autoconnect on or off), `WifiRadio {enabled}`,
    `WifiConnect` with `hidden`; responses `WifiNetworks` (at most 20),
    `WifiProfiles` (at most 16) and a `WifiState` extended with radio,
    signal, security and IPv4 address; every response within 4 KiB.
W2. Owner: requests run outside the shared owner state. Scan asks
    NetworkManager for a new scan and returns the current list (strongest
    first, one entry per name). Connect writes a mode-0600 keyfile with its
    own `id` and `uuid`, the SSID as bytes and the key management the network
    announces (open, WPA2-PSK, WPA3-SAE; WEP and enterprise refused), loads
    it and brings it up within 30 s; a failed profile is removed; one profile
    per network name. Disconnect acts on the device. Saved profiles can be
    connected, forgotten and switched to automatic connection; the radio can
    be switched. The password never reaches a command line, a log, a response
    or the saved list.
W3. Server: `GET /api/network/wifi`, `GET /api/network/wifi/scan`,
    `GET /api/network/wifi/saved`, `POST /api/network/wifi/saved`
    (`{id, action}`: connect, forget, auto-join on or off),
    `POST /api/network/wifi/connect`, `POST /api/network/wifi/disconnect`,
    `POST /api/network/wifi/radio`, gated by the `network_wifi` capability;
    a 45-second client for Wi-Fi requests.
W4. Web (Settings > System > Network): radio switch; the current network with
    signal, security, address and Disconnect; networks in range with signal,
    lock and Refresh, a password dialog (none for open networks); "Other
    network" for a hidden one; saved networks with Connect, automatic
    connection and Forget; English and Russian; desktop and mobile; a
    contract test on a mocked API.
W5. Board: scan lists the networks in range; radio off and on; a wrong
    password fails and leaves no profile; Ethernet stays the default route.
    Joining a real network needs the operator: a test network's name and
    password stored in project-memory, or the operator joins it in the page;
    then connect, reconnect after reboot, automatic connection off, forget.
W6. Audit, design document (`docs/design/wifi-v1.md`), release, `check.sh`.
W7. Added at the operator's request on 2026-09-27: the connected network's
    details under it (IPv4 address, gateway, resolvers, IPv6 address, MAC,
    channel and band, link speed).
W8. Added at the operator's request on 2026-09-27: Wi-Fi takes over when the
    Ethernet cable is out (checked on the board), and a switch chooses the
    main connection, Ethernet or Wi-Fi, with the other as standby; the
    change is reverted unless the page confirms it. Aggregating both links
    is not offered (see the design document).

## Order and operator steps

F1-F5 first (they fix a failure the page shows today and a HID stall), then
W1-W6. Operator: credentials of a test Wi-Fi network for W5 (or joining it
in the page); the phone check of both sections at the end.
