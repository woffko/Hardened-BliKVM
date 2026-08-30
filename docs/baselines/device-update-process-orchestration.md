# H616 candidate-process orchestration gate

The designated test-only BliKVM H616 device passed a temporary, isolated
candidate-process gate using the exact static-PIE artifacts from source commit
`2be3f7ff767ae9ed3fa07d843310d667ca7dd418`:

```text
bli-kvm-server                  2cda3b944f53b0aef56af7140030afd9c2834a2739346c64e69702ddb333a7a9
bli-kvm-update-supervisor       e6dd0b2396fd230830cb87d1a5b14bde03436df4b3be12d01b612b3fc87edee4
bli-kvm-update-health-reporter  d4d89d3ec59bbd368980a2975ac337771962f4002478b178a1c6a3ea3d0c201d
```

Two independent AArch64 target directories produced the same six release
binaries. Their complete deterministic prototype archives were byte-for-byte
identical:

```text
hardened-blikvm-prototype-0.1.0-dev-aarch64.tar.gz
e7e7ffa6e43007fcf0f9fdf2f864fcf2281553ec44ba75d3c1883f06c48ca674
```

## Accepted behavior

- a confirmed signed slot started the real Rust server on one isolated
  loopback port;
- sequence 2 was activated as a candidate and its strict `/api/health`
  response was validated;
- systemd delivered the one-use token through `LoadCredential` to the
  unprivileged Rust health reporter;
- the reporter sent exactly 32 raw bytes to the socket-activated confirmation
  service, which raised the confirmed anti-rollback floor to sequence 2;
- the confirmation sandbox could write only the marker-bound state and runtime
  directories; the signed slot tree remained read-only;
- successful confirmation consumed the token;
- an intentionally non-executable sequence-3 candidate failed to start and did
  not become confirmed;
- the watchdog rolled it back after the health timeout, and the restored
  sequence-2 server passed the same health contract;
- every transient unit, socket, runtime directory, staging tree, and alternate
  listener was removed.

The first sandbox mirror omitted the two exact writable paths while using
`ProtectSystem=strict`. Stable manager exit status 89 identified the resulting
journal I/O refusal without exposing a path, token, key, or state document.
Adding only the state and runtime paths made the complete gate pass. Removing
concurrent status polling did not change the failure, so lock contention was
not the root cause. The packaged confirmation, activation, and watchdog units
now carry equivalent exact allowlists; they remain uninstalled and unenabled.

## Preserved device state

KVMD and kvmd-nginx were active before, during, and after the accepted run.
Neither service was restarted. The root filesystem began and ended `rw` as
requested. No HID, virtual-media, ATX, Power, Reset, reboot, or other physical
action was invoked. The private receipt is mode 0600 and contains only fixed
booleans, coarse states, sizes, and hashes.

## Remaining boundary

The gate used unique `/tmp` staging and transient `/run/systemd/system` units.
It did not install or enable the packaged units, enroll a production signing
key, expose an update API or GUI, interrupt a real storage write, reboot or
power-cut the device, validate a full-system image, or establish an H616 A/B
boot/recovery path.
