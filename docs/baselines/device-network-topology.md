# Sanitized H616 network topology baseline

The network source audit is pinned to
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea` and its exact web
submodules `blikvm-web-server@932a47740020268ba89c254809d175a371944681`
and `blikvm-web-client@aa826d9c8b2d7d9367a2ca2abbc61f0abf9084bb`.

The vendor backend combines narrowly useful interface-type/state inventory
with private topology disclosure and root mutations:

- `/api/ip/interfaces` shells out to NetworkManager for interface names, types,
  and state;
- `/api/ip/:iface` additionally returns connection names, addresses, gateway,
  and DNS data, which are outside the public baseline;
- Wi-Fi status/scan returns SSID and BSSID data, while scan actively rescans;
- Wi-Fi connect interpolates supplied values into a shell command, recursively
  changes connection-profile permissions to 0777, and restarts NetworkManager;
- static/DHCP changes may remount the root filesystem, modify profiles, cycle
  the selected connection, and have no connectivity watchdog or rollback;
- web protocol/port changes rewrite configuration and terminate the server.

None of those routes or command helpers is accepted as a Hardened runtime
boundary.

On 2026-08-31, a commit-bound metadata-only probe from source `1e96605`
inspected four fixed interface candidates through `/sys/class/net` and queried
only service active state. It did not invoke `nmcli`, `ip`, `ifconfig`,
`networkctl`, Wi-Fi scan, connection/profile inspection, or any mutation.

Sanitized live facts:

- three total interfaces exist, including loopback;
- `eth0` is Ethernet type 1, `up`, carrier present, using `dwmac-sun8i`;
- `wlan0` is wireless type 1, `down`, carrier absent, using `rtl8723ds`;
- fixed candidates `end0` and `usb0` are absent;
- NetworkManager is active and the `nmcli` executable exists;
- systemd-networkd, ConnMan, and dhcpcd are inactive;
- the root filesystem remained in its existing read-write state.

The private mode-0600 receipt explicitly records that no IP or link-layer
address, prefix, route, resolver, SSID, BSSID, connection profile, endpoint,
credential, or unique identifier was read. No filesystem, interface, service,
manager, or physical state changed.

This establishes current interface classes, drivers, link state, and manager
ownership only. There is no Rust network inventory adapter or network mutation
capability. Future configuration requires strict typed data, exact interface
eligibility, secret-safe Wi-Fi handling, privilege separation, a commit/confirm
journal, connectivity watchdog, bounded automatic rollback, out-of-band
recovery, repeated apply/reboot tests, and no management-address disclosure in
public evidence.
