# H616 control-denial checkpoint

On 2026-08-30, the denied-by-default HID/ATX/virtual-media boundary passed on
the designated test-only H616 device. Two independent AArch64 builds produced
byte-identical archives with SHA-256
`ae987cbc70503b921c926063bf94bc45a871913ae81afdaa2051f56340efd90d`.
The source commit was `1f79b87b24a0b25c9a3b56270e7bd02f3b1c754f`, and the
static-PIE control broker binary SHA-256 was
`f7636c200b8a828067e13dd5732e3a3860c462d333cd1c7208e0d32388ec5111`.

The control protocol is separate from the read-only inventory protocol. It has
fixed HID report shapes, enum-only ATX actions, opaque virtual-media image IDs,
bounded framing, strict unknown-field rejection, request correlation, and a
mandatory 128-bit operation ID. The broker in this checkpoint links no KVMD,
HID gadget, GPIO, configfs, block-device, path, URL, or shell backend.

Verified results:

- the control broker ran as a non-root identity with no supplementary `kvmd`
  group, no credential environment, no capabilities, and `NoNewPrivileges`;
- a server-like non-root identity reached only its mode-0660 Unix socket;
- a zeroed keyboard release report, an ATX enum request, and a disconnected
  virtual-media request each returned exactly `capability_disabled` with the
  matching request and operation IDs;
- no control request or WebSocket was exposed by the authenticated browser UI;
- the existing TLS, cookie, inventory, read-only MJPEG decode/reconnect,
  password-rotation, stream-revocation, and logout gates still passed;
- the control broker, video proxy, server, read broker, Nginx, certificate,
  tunnel, ports, and marker-protected staging were removed;
- KVMD and KVMD Nginx remained active, the root filesystem remained in its
  pre-existing read-write state, and no persistent path changed;
- no HID injection, GPIO access, media mutation, physical Power, or physical
  Reset action was possible or invoked.

This checkpoint accepts protocol parsing, process isolation, packaging, and
denial behavior only. It does not accept a HID lease, report translation,
pressed-key release on disconnect, an ATX arming/cooldown backend, a media
catalog/configfs backend, any server control route, or any control capability.
