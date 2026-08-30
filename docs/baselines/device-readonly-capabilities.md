# H616 read-only control-capability checkpoint

On 2026-08-30, the source-only HID, ATX, and virtual-media transition work was
checked without enabling or invoking any control backend. Two independent
AArch64 builds produced byte-identical archives with SHA-256
`62bcb462bf094eb8848bd40c938374533f7da7622c0dee7f3e81fb6c40aa3b95`.
The source commit was `90fa26ea44dfede0f2f06d8cf40cbfdca909570c`.

The full AArch64 workspace build compiled the HID translator/lease/ledger,
fixed KVMD HID client, persisted ATX policy, fixed KVMD ATX client, opaque media
catalog, persisted media policy, and fixed KVMD media client. These libraries
were not linked into the production control broker, which remained
denial-only with no backend or credential.

Verified H616 Chromium results:

- video capability was `read_only` and Chromium decoded/reconnected MJPEG;
- HID capability remained `unavailable`;
- Power action capability remained `unavailable`;
- power LED observation was `read_only`;
- virtual-media state was `read_only`;
- mutating menu/settings logic required `read_write`;
- nineteen API responses contained no HTTP 404;
- the browser issued zero control requests and zero WebSocket connections;
- HID, ATX, and media control-protocol probes each returned
  `capability_disabled` from the no-backend broker;
- TLS, cookies, inventory, password rotation, stream revocation, and logout
  continued to pass;
- all temporary processes, TLS material, ports, and marker-protected staging
  were removed in the final check;
- KVMD and KVMD Nginx remained active, the root filesystem remained in its
  pre-existing read-write state, and no persistent path changed;
- no HID injection, media mutation, physical Power, or physical Reset action
  was invoked.

This checkpoint accepts read-only capability semantics, fail-closed GUI
gating, local policy/adapter tests, and AArch64 compilation only. It does not
accept runtime wiring of the HID/ATX/media libraries, safe-target attestation,
HID event injection, ATX timing or polarity, media attach/eject, BIOS boot,
restart/reboot recovery, or physical behavior.
