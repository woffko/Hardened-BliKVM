# Packaging boundary

These files describe the intended service boundary; they are not installed by
the host development build.

`hardened-blikvm-platform-broker.service` has no TCP address family, no device
namespace access, no capabilities, and no writable path outside its runtime
directory. During the KVMD transition it joins the `kvmd` supplementary group
only to connect to `/run/kvmd/kvmd.sock`.

`hardened-blikvm-server.service` runs under a different non-root identity. It
can use only Unix sockets and IPv4 loopback, may bind only TCP port 18080, and
has no device or capability access. Its only persistent writable path is the
systemd-managed mode-0700 state directory. Secure cookies assume an HTTPS
reverse proxy; the Rust process itself remains loopback-only.

The two files below must be provisioned locally as root-owned mode 0400 files
and must never be included in a package, image source tree, command line,
environment variable, log, or support bundle:

- `/etc/hardened-blikvm/kvmd-user`
- `/etc/hardened-blikvm/kvmd-password`

They should identify a dedicated KVMD account, not an administrator or SSH
account. The service loads them with systemd `LoadCredential`; the broker
accepts no credential arguments and has no compiled default.

Creating that KVMD account and installing these files are separate on-device
mutations. They require explicit authorization, `rw`, backup/rollback, and a
reboot/recovery gate. `tools/stage_device_systemd_gate.py` exercises only a
temporary test account and runtime units with a watchdog, then requires a
byte-identical htpasswd restore and removes all staging. It is not a production
installer or permanent account-provisioning tool.
