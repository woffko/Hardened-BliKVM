# H616 read-only thermal observation checkpoint

On 2026-08-31, the bounded H616 SoC temperature slice passed its host,
reproducibility, reversible systemd, and real-browser gates. The accepted
development bundle is:

- source commit: `329194e68537a774ae4e8d909617f92851d977d0`;
- archive SHA-256:
  `43b0709a73417e1c6a4f8f44cad61fa823f0f24055d8bcf7a03ac07cc3983f88`;
- archive size: 4,768,881 bytes;
- AArch64 server SHA-256:
  `2fcf8013f5d8d58554bcd578d355e7bb3b8ddff682330a417e6922cb2e35d85b`;
- AArch64 platform-broker SHA-256:
  `a6b1eb9cf6fb074b758f2cc3e7a2afc14c452422769b08a1d7f4f027fd7c03a3`;
- AArch64 video-proxy SHA-256:
  `9e21bb47006ea0652af2e5144427a1b0e5cb141cba5301e16e2d09cc2b9b48fe`;
- web tree SHA-256:
  `0034e70e31a784dc6c70dff8a2f7c9abe18748d8738efb3e1bd0c4922b64e0b6`.

Two fresh and independent AArch64 target/output directories produced
byte-identical archives. Their manifests identify the same committed source,
lock hashes, six shipped Rust payload hashes, and web tree. Both manifest
acceptance booleans remain false: this is a development checkpoint, not a
firmware or release artifact.

## Implemented boundary

Runtime-inventory schema v2 adds a required thermal object whose value is a
nullable integer in millidegrees Celsius. The H616 adapter opens only the
compiled `/sys/class/thermal/thermal_zone0/temp` source with final-component
no-follow semantics. It rejects non-regular or multiply linked files,
group/world-writable metadata, oversized or non-ASCII input, and values outside
-40,000 through 150,000 m°C. Errors become only a null observation and a coarse
local class; no path, raw input, or backend detail enters the public API.

The capability manifest exposes only read-only observation with runtime
evidence. Fan, PWM, GPIO, and all other board mutations remain unavailable.
The React interface formats the observation to one decimal degree Celsius in
Settings / About; invalid or unavailable values render as `-`.

The full host check passed, including workspace formatting, check/test/clippy,
source and systemd boundaries, 55 Python tests, BasedPyright, the production
TypeScript build, ESLint, and Prettier. Regression tests additionally prove
that a disconnected MJPEG downstream releases its bounded proxy permit even
when upstream is silent, while the HTTP client keeps its request half open for
the lifetime of a streaming response.

## Device gates

The final bundle passed the dedicated `thermal-only` systemd profile on the
designated test-only H616 device:

- bundle and every payload checksum matched;
- a temporary dedicated KVMD account, ten-minute rollback watchdog, and
  systemd `LoadCredential` boundary were active;
- broker, video-proxy, and server units ran without capabilities and with their
  expected network, device, UID/GID, socket, and credential separation;
- setup and login returned HTTP 200;
- 20 authenticated sanitized inventory requests returned HTTP 200, schema v2,
  platform backend `kvmd_transition`, and a bounded 56,681 m°C observation;
- thermal capability was `read_only` with `runtime_verified` evidence;
- video unit sandbox checks passed, while video stream, reconnect, and
  revocation fields were explicitly `skipped` by this focused profile;
- the original htpasswd content was restored byte-for-byte, the temporary
  account, units, runtime and staging paths disappeared, the watchdog stopped,
  both existing KVMD services remained active, and the root filesystem remained
  read-write.

The same archive then passed the TLS Chromium gate:

- TLS 1.2 and 1.3 reached the device-loopback Nginx through strict-host-key SSH
  forwarding;
- secure HttpOnly session and readable CSRF cookies used Strict SameSite;
- Chromium observed inventory schema v2, the bounded thermal observation, and
  the runtime-verified read-only capability;
- Settings / About rendered the value derived from its exact inventory
  response;
- MJPEG decoded, four reconnects and session revocation passed;
- password rotation and logout passed with zero control requests and zero WebSocket
  connections;
- all temporary processes, ports, and staging were removed, KVMD services stayed
  active, and the root filesystem remained read-write.

No HID, ATX, physical Power/Reset, GPIO, fan, PWM, media mutation, update,
partition write, reboot, or flash action was invoked.

## Remaining limits

The observed temperatures are point-in-time functional evidence, not sensor
calibration or a thermal-soak result. Missing-sensor behavior has host tests but
still needs a controlled live fault gate; permanent packaging and reboot
persistence also remain open.

The broad systemd video-throughput profile is deliberately not marked passed on
this source commit. During this session the current upstream supplied fewer
than two JPEG starts in its three-second capture window, so that independent
gate failed closed and rolled back. The final TLS gate still proved decode,
four reconnects, and revocation, but it does not replace the separate
two-client, frame-rate, hotplug, latency, and soak matrix.
