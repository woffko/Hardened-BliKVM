# System actions and network writes (E3)

Status: design for plan phase E3 (`docs/design/gui-parity-plan.md`). Session
auto-lock is already implemented and covered by server tests; this document
covers the privileged remainder: reboot and shutdown, the memory and swap
limits, and the network writes.

## What exists already

- `bli-kvm-system-action-policy` models reboot and shutdown with confirmation
  bindings, an origin boot binding, boot-uptime guards, a cooldown, a bounded
  journal and post-boot acceptance. `bli-kvm-system-action-journal` and
  `bli-kvm-system-action-store` persist it, and
  `bli-kvm-system-action-maintenance` dispatches it.
- The device already accepts both privileged halves:
  `device-system-action-store-native.json` and
  `device-system-action-maintenance.json` (`result=passed`), staged by
  `tools/stage_device_system_action_store.py` and
  `tools/stage_device_system_action_maintenance.py`.
- `/api/vm/session-lock` implements the idle auto-lock and the forced lock.

What is missing is the request surface that turns an operator confirmation into
a prepared, journaled action, and the capability that advertises it.

## Request surface

The privileged component stays the board owner; the server never writes the
journal or runs the action itself.

1. **Wire request** (`bli-kvm-core::control_v2`): a `SystemAction` variant
   carrying the action (`reboot` or `shutdown`) plus the same confirmation
   material the ATX path uses (`session_binding`, `confirmation_binding`,
   `confirmed_at_unix_ms`). The owner derives the operation id from the
   operation id in the envelope and the origin boot binding from the running
   boot, so a request prepared before a reboot cannot be replayed after it.
2. **Broker profile** (`bli-kvm-control-broker`, feature
   `system-action-profile`): holds the policy engine, the journal path and an
   `ActionRunner`. Eligibility is built from explicit flags
   (capability enabled, backend ready, local admin, reauthentication,
   confirmation) and from the environment guards (update, storage, recovery,
   post-boot observer) exactly as the policy expects; a rejection maps to a
   fixed protocol code. Dispatch records the intent first and then asks the
   runner. Two runners exist: `RefusingRunner` (the default) and `ExecRunner`,
   which executes one operator-supplied absolute command per action with **no
   arguments**, so a request cannot inject one and nothing runs unless both
   paths are configured.

   The board build compiles the profile in and the owner attaches it to the
   same state that serves the gadget, media and ATX: with
   `BLI_KVM_SYSTEM_ACTION_PROFILE=1` the owner runs
   `run_with_chardev_board_profile_with_system_action`, which is otherwise the
   identical board profile; `BLI_KVM_SYSTEM_ACTION_ARMED=1` is the second,
   separate switch the endpoint capability follows, and the two commands come
   from the optional operator file
   `/etc/hardened-blikvm-gadget/system-action.env` (sample shipped in the
   bundle). Nothing executes while the file is absent, and the origin boot
   binding comes from the running boot id, so a request prepared before a
   reboot cannot be replayed after it.

   A deploy check caught a real regression here: with the profile feature
   compiled in, an initial wiring skipped the plain board run whenever the
   profile environment flag was unset, so the owner exited immediately. The
   entry point now always falls through to the plain board profile.
3. **Server endpoints** (`POST /api/vm/system/reboot`, `/shutdown`): the
   settings UI's confirm action, gated by the usual session, CSRF and origin
   checks plus the `system.actions` capability, which stays `unavailable` in
   shipped builds and is advertised only when the operator arms the surface
   (`BLI_KVM_SYSTEM_ACTIONS=1` in a restricted build with a control socket).
   The POST forwards the operator confirmation, refuses a request without it
   before any IPC and maps the broker response.
4. **Post-boot acceptance** (decided by the operator 2026-09-16): the journal
   is persisted through `bli-kvm-system-action-store` - the owner loads the
   state at startup and writes it back after every prepare and dispatch - and a
   one-shot boot unit runs the maintenance binary with a new `accept-post-boot`
   command, which records the post-boot observation and calls `accept_post_boot`
   only when the boot identity changed and the guards are healthy. A pending
   entry blocks another action; it is never retried automatically. Until this
   lands the device gate reports the dispatch and the boot change but not the
   acceptance.

## Memory and swap

- Reads stay in the server (`/api/vm/memory/limit`, `/api/vm/swap` views from
  `/proc/meminfo` and the zram sysfs).
- Writes are bounded owner actions: the swap size is the reviewed zram
  configuration and the memory limit is a bounded integer in megabytes, both
  validated against the operator-configured bounds and read back after the
  write. They reuse the same confirmation and journal path as reboot, so a
  failed write leaves an explicit journal phase instead of a silent change.

## Network writes

- DNS servers, IPv6 enable/disable and the interface list are the first
  targets; firewall, SSH, mDNS and Wake-on-LAN follow only where the hardware
  and the installed stack support them (`wlan0` exists on this board, the
  Ethernet is `dwmac-sun8i`, and NetworkManager is the active manager).
- Every write goes through a bounded owner action with read-back, and the
  capability advertises only what the board can actually do. Interactive wifi
  association is explicitly out of scope until a reviewed helper exists.

## Gates

1. **Host**: policy/broker tests with a mock runner (disabled capability,
   confirmation window, duplicate operation, refusal mapping) and server tests
   for the endpoints with the capability hidden and enabled.
2. **Device, operator confirmed**: arm the profile, request one reboot with a
   confirmation, verify the journal reaches `PostBootAccepted` after the boot
   and that the guards hold; the same run records the memory/swap write and
   read-back. Receipt `system-action-gate-*.json`, listed in the acceptance
   manifest.
3. **Rollback**: disarming the capability is a configuration change; the
   journal keeps the history and the blocked state is visible in the status.
