# Security boundaries

## Secret handling

- Test-device routing and credentials belong only in encrypted Project Memory.
- Do not write credentials to Git, command arguments, environment variables,
  logs, evidence bundles, update archives, or delegated prompts.
- Prefer one-time secret stdin handles for reviewed non-interactive device
  operations.
- Never import accounts, SSH material, history, cookies, or tokens from a field
  image into a BLi-KVM release.

## Network process

`bli-kvm-server` is unprivileged and owns HTTP policy only. It must not receive
GPIO, configfs, raw block, arbitrary file, or arbitrary process permissions.
The development server binds to loopback and refuses a public bind. Passwords
are sent only over the TLS/reverse-proxy boundary; BLi-KVM
does not use a fixed browser-side key as reversible password obfuscation.

## Privileged broker

The broker accepts typed operations over a permission-restricted Unix socket.
Its transition backend connects to KVMD only over `/run/kvmd/kvmd.sock` and
receives a dedicated KVMD credential through systemd credentials. The
network-facing server never receives that credential. Native backends use fixed
device paths, strict field validation, bounded timeouts, and an operation
allowlist. No backend may invoke a shell with request-derived content.

KVMD's public Nginx routes use `/api/<route>`, but the local Unix socket uses
the rewritten internal `/<route>` form. The transition client therefore
allowlists only `/info`, `/atx`, `/hid`, `/msd`, and `/streamer`; it also sends
a fixed Host header required by HTTP/1.1. Endpoint-level diagnostic logs expose
only a fixed endpoint enum and coarse error class, never a response body or
credential.

Read-only observations and mutating methods are separate interfaces. HID, ATX,
media, GPIO, or configfs methods stay unavailable until their adapter and
physical gate pass.

## Media processes

Transition uStreamer and Janus processes bind to loopback or Unix sockets and
run under dedicated service identities. Only the authenticated Rust server is
network-facing.

## Filesystem and update boundary

- Read-only inspection never enables write mode.
- A write requires explicit task authorization, preflight, rollback, and a
  separate maintenance transition.
- Application and system update trust are separate.
- No NanoKVM raw partition path or update assumption is valid for H616.
- Production release requires signature rejection tests, power-loss recovery,
  and recoverable physical media.
