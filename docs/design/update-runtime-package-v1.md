# Inert application-update runtime package v1

## Purpose

`hardened-blikvm-update-runtime` is a separate deterministic `arm64` Debian
package for the Rust application-update process boundary. It depends on the
exact accepted `hardened-blikvm-runtime` revision but does not grant update
authority or alter the running application when installed.

The package can be built and installed before a production signing-key
ceremony. This makes binary, filesystem, systemd, package-manager, and rollback
behavior testable without inventing a production public key or ever handling a
production private key.

## Exact payload

The package contains only:

- `bli-kvm-update-supervisor` and `bli-kvm-update-health-reporter` under the
  existing hardened libexec directory;
- disabled watchdog, timer, confirmation socket/service, activation, fixed
  apply, confirmed-server, and candidate-server systemd templates;
- the public application-update and activation protocol documents; and
- sysusers/tmpfiles source rendered only as documentation examples.

The update identity is already part of the accepted fixed runtime identity
set. The package does not install an active sysusers or tmpfiles fragment and
has no maintainer script, trigger, conffile, or post-install action.

## Authority that remains absent

Installation must leave all of the following absent:

- `/etc/hardened-blikvm/update-supervisor.json` and every application public
  trust key;
- application slot, update state, update runtime, health-token, deletion
  marker, and current-pointer roots;
- enabled or active update services, timers, sockets, confirmed servers, or
  candidate servers;
- an update upload/check/activate HTTP route or GUI control;
- a signing private key, credential, URL, system-image writer, boot selector,
  or hardware-mutation backend.

The ordinary persistent server and Hardened Nginx continue to own the accepted
runtime and public listener. Package installation may not restart them, KVMD,
uStreamer, or any target device.

## Build and device gates

The deterministic package manifest binds package version, exact runtime
dependency, source bundle hash, source commit, builder commit, architecture,
and every absent authority above. Two builds from independent but byte-equal
prototype bundles must produce byte-identical packages.

The first H616 gate is install-only and rollback-protected. It must verify the
complete package file tree, modes, root ownership, lack of file capabilities,
all update units disabled/inactive, absent configuration/state/slots/runtime,
unchanged account/TLS/Nginx hashes, unchanged running service PIDs and restart
counters, exact public listener ownership, and clean `dpkg --audit`. It then
retains the inert package and independently repeats the checks in a new SSH
session. The independent `--verify-only` path consumes the accepted private
receipt only to bind the original PID/restart/account/configuration/TLS facts;
it performs no upload, package-manager operation, unit transition, or staging.

Production public-key enrollment, state/slot initialization, unit activation,
signed candidate staging, interruption, repeated reboot, rollback, update GUI,
and release acceptance remain separate later gates. ATX Power/Reset and other
attached-target power-control actions are not part of this package or its
device tests.
