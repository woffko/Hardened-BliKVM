# Sanitized H616 hardware baseline

This document contains no endpoint, username, credential, host identity, or
private configuration. Those values remain in encrypted Project Memory.

## Current user-reported test baseline

- Hardware family: BliKVM v4, Allwinner H616, ARM64.
- Runtime: PiKVM/KVMD on an Armbian-derived system.
- Current KVM functionality: reported working by the device owner. A bounded
  authenticated read-only API probe now verifies the running control-plane
  status listed below; physical interaction verification is still pending.
- Root filesystem: the latest SSH preflight found it already mounted `rw`
  before BLi-KVM performed any device mutation. The owner explicitly chose to
  leave this test device `rw` for the current work. Gates record the mode and
  do not call the `rw` or `ro` helpers unless a later task explicitly requires
  a transition.
- Public reachability check: HTTPS redirects to the PiKVM login boundary and
  protected API access rejects unauthenticated requests.

## Authenticated read-only runtime baseline

Captured without entering filesystem write mode:

- architecture `aarch64`, kernel `5.19.4-sunxi64`, live userspace Armbian
  25.11.2 Jammy;
- KVMD version `3.291`;
- streamer version `6.11` and capture source online;
- HID subsystem online and not busy;
- ATX subsystem enabled and not busy, with power LED asserted at capture time;
- virtual-media subsystem enabled, online, not busy, and not connected at
  capture time;
- Janus, Janus static, VNC, and IPMI extras disabled;
- web terminal extra enabled.
- KVMD and its Nginx entrypoint active;
- KVMD Unix socket present with mode 0660 and `kvmd:kvmd` ownership;
- `/tmp` executable with approximately 512 MB available; `/run` mounted
  `noexec`;
- systemd 249 and no `systemd-creds` CLI, although plain `LoadCredential`
  support is available in this systemd generation;
- both `rw` and `ro` helper commands present.

This proves API/runtime availability only. It does not prove keyboard events,
mouse coordinates, ATX pulse timing, media bootability, video quality, or any
other physical interaction.

A separate metadata-only GPIO profile now confirms the pinned chip0 offsets
and current consumers without requesting lines or reading values. Power/Reset
and ATX LED lines are owned by KVMD, activity LED and fan by legacy sysfs, and
the display-button line is unused input. This is ownership evidence, not
physical polarity or behavior acceptance.

A separate metadata-only serial probe found no ttyUSB0-3 class/device nodes and
no serial/by-id directory. It opened no TTY and read no unique identifiers or
frames. Serial-switch runtime support remains absent until deliberate hardware
is attached and physically gated.

A metadata-only display probe confirms that the vendor-named H616 `OLED` path
is an ST7789 240x240 LCD on root-only `/dev/spidev1.0`, backed by the `sun6i-spi`
controller. Source-associated control offsets 259, 260, and 262 are already
used legacy sysfs outputs. No SPI node or GPIO value was opened, no transfer or
line request occurred, and the display capability remains hidden.

A sanitized network probe confirms NetworkManager ownership, an active H616
Ethernet interface and an inactive wireless interface through sysfs metadata
only. Addresses, MACs, SSIDs/BSSIDs, routes, resolvers, profiles, endpoints and
credentials were not read or retained. No network command or state change
occurred; network configuration remains unavailable.

## Field-image evidence

- MBR partition layout with separate root, data, and virtual-media roles.
- Root filesystem observed as Armbian 24.2.1 Jammy with a PiKVM/KVMD stack;
  it is older than the live 25.11.2 userspace and is not a live-state clone.
- Vendor source and KVMD installation were materially older than current
  upstream.
- The captured raw image ends 866,123,776 bytes before its fourth partition,
  so it is not a valid recovery or redistribution artifact.
- Persistent device state was present. Secret-bearing files were not read and
  extracted analysis data remains only in the user-designated temporary D:
  workspace outside Git.

## Physical gates not yet executed by BLi-KVM

- video source/no-signal/hotplug and mode matrix;
- keyboard and absolute/relative mouse behavior;
- ATX action timing and LED polarity;
- configfs topology and BIOS boot from virtual media;
- OLED/display behavior, fan, buttons, and serial switch; read-only display
  topology is recorded separately;
- network apply/rollback, Wi-Fi privacy, connectivity watchdog, recovery and
  reboot persistence; sanitized ownership topology is recorded separately;
- RTC calibration, drift, battery retention, alarm/wakeup, and clock-write
  direction; read-only kernel topology is recorded separately;
- fan GPIO electrical polarity, PWM semantics, minimum spinning duty, exclusive
  owner handoff, stall/fault behavior and thermal soak; read-only line ownership
  is recorded separately;
- update interruption, recovery card, and repeated reboot tests.
