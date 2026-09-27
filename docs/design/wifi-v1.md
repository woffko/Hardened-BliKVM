# Wi-Fi v1

The board carries an RTL8723DS on `wlan0` managed by NetworkManager. The
reference (Hardened NanoKVM) has no Wi-Fi section; this one is ours. Ethernet
remains the board's main connection: Wi-Fi is an additional client link, never
an access point (the reference's access-point request is refused with
`unsupported_mode`).

## Boundaries

- The web server holds no NetworkManager access. Every action is a typed
  control request to the board owner (`crates/bli-kvm-control-broker/src/
  network_settings.rs`), which runs `nmcli` with fixed arguments.
- Gating: the `network_wifi` capability is `read_write` only in a
  restricted-control build with the operator's opt-in; otherwise the server
  answers `503` and the owner refuses writes with `capability_disabled`.
- The owner serves Wi-Fi requests outside its shared state, on its own lock
  and a blocking worker, so an association (up to 30 s) never holds HID or
  media requests. The server uses a 45-second control client for Wi-Fi.
- A password travels once, in the connect request body, to the owner, which
  writes it into a mode-0600 keyfile. It never appears on a command line, in
  the environment, a log, a response, or the saved-network list.

## Control protocol

Requests (`PlatformControlRequestV2`):

| Request | Validation | Owner action |
| --- | --- | --- |
| `wifi_state {}` | none | `nmcli device`, `radio wifi`, the in-use scan entry, `IP4.ADDRESS` |
| `wifi_scan {rescan}` | none | optional `device wifi rescan`, then `device wifi list --rescan no` |
| `wifi_saved {}` | none | `connection show` plus each Wi-Fi profile's SSID |
| `wifi_connect {ssid, password, hidden}` | SSID 1-32 bytes without control characters; password empty, 8-63 printable characters, or 64 hex digits | keyfile, `connection load`, `--wait 30 connection up uuid` |
| `wifi_disconnect {}` | none | `device disconnect wlan0` |
| `wifi_profile {id, action}` | canonical UUID; action `connect`, `forget`, `autoconnect_on`, `autoconnect_off` | the UUID must be a saved Wi-Fi profile; `connection up`, `delete` or `modify connection.autoconnect` |
| `wifi_radio {enabled}` | none | `radio wifi on/off` |

Responses: `wifi_state` (available, up, interface, SSID, state, radio, signal,
security, IPv4 address), `wifi_networks` (at most 20) and `wifi_profiles` (at
most 16). The limits keep the widest possible answer, every SSID byte escaped
by JSON, within the 4 KiB control message; a protocol test encodes that case.
Security labels are `open`, `wpa2` (WPA1 and WPA2 personal), `wpa3`,
`wpa2_wpa3`, `wep`, `enterprise` and `other` (OWE and anything unknown).

## Joining a network

1. The SSID must be in the current scan, unless the operator names a hidden
   network. Its announced security selects the key management: none for open,
   `wpa-psk` for WPA2 and WPA2/WPA3 transition, `sae` for WPA3-only. A hidden
   network is WPA2-personal with a password and open without. Enterprise, WEP
   and other security are refused, as are an open network with a password and
   a secured one without.
2. The owner writes `hardened-blikvm-wifi-<uuid8>.nmconnection` with
   `create_new`, `O_NOFOLLOW` and mode 0600 into the real
   `/etc/NetworkManager/system-connections` directory: its own `id` and
   `uuid`, `interface-name=wlan0`, the SSID as its byte list (any name reads
   back exactly), `hidden=true` when asked, the key management and the
   GKeyFile-escaped passphrase, and automatic IPv4 and IPv6.
3. `nmcli connection load` then `--wait 30 connection up uuid`. On failure
   (for example a wrong password) the new profile is deleted and its file
   removed; an earlier profile for the same name stays.
4. On success, earlier profiles for the same name are deleted: one profile per
   network name.

A network in range that has a saved profile is rejoined from the list without
asking for the password again.

## Server routes

All under `/api/network/wifi`, authenticated, CSRF-checked for writes:

| Route | Body or query | Answer |
| --- | --- | --- |
| `GET /api/network/wifi` | - | `{available, connected, ssid, interface, state, radio, signal, security, address}` |
| `POST /api/network/wifi` | any | `{applied: false, rejection: "unsupported_mode"}` |
| `GET /api/network/wifi/scan` | `?rescan=true` | `{networks: [{ssid, signal, security, channel, inUse}]}` |
| `GET /api/network/wifi/saved` | - | `{profiles: [{id, ssid, autoconnect, active}]}` |
| `POST /api/network/wifi/saved` | `{id, action}` | `{applied, rejection?}` |
| `POST /api/network/wifi/connect` | `{ssid, password, hidden?}` | `{applied, rejection?}` |
| `POST /api/network/wifi/disconnect` | - | `{applied, rejection?}` |
| `POST /api/network/wifi/radio` | `{enabled}` | `{applied, rejection?}` |

The server checks the SSID, password and profile ID before calling the owner.

## Web

Settings > System > Network shows the Wi-Fi block when the board reports a
Wi-Fi device: the radio switch with the current network or "Not connected";
the connected network with signal, IPv4 address, security and Disconnect;
networks in range, strongest first, with signal, a lock for secured ones and
a tag for security the board cannot join, plus Refresh (a new scan, read again
four seconds later); "Other network…" for a hidden network; saved networks
with auto-join, Join and Forget. A refused join keeps the password dialog open
with the reason. Read-only access shows the same state with every control
disabled. English and Russian texts; other languages fall back to English.
`tools/playwright/wifi_contract.mjs` covers the block on a mocked API, desktop
and mobile.

## Board verification

On the test board: the scan lists the networks in range; the radio goes off
and on; a wrong password fails and leaves no profile; Ethernet stays the
default route while Wi-Fi is connected. Joining a real network, reconnecting
after a reboot, turning auto-join off and forgetting need a test network's
credentials from the operator (stored in project memory) or the operator
joining it in the page. Evidence goes under `evidence/private`.
