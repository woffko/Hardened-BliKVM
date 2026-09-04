# Runtime identity and credential provisioning gate design

Status: design and host implementation boundary only. The retained inert
package is accepted on H616, but no permanent service identity or dedicated
KVMD credential has yet passed this gate. This design does not authorize
service start/enablement, Nginx activation, reboot, update trust, diagnostic
persistence, or hardware action.

## Declarative identity set

Provisioning consumes only the fixed
`packaging/sysusers.d/hardened-blikvm.conf` source whose SHA-256 must match the
documentation copy already installed by the exact retained package. No caller
may supply another identity file, account name, numeric UID/GID, home, or
shell. The complete current template is applied rather than a rewritten
subset:

- one `hardened-blikvm` shared group;
- `hardened-blikvm-broker`;
- `hardened-blikvm-video`;
- `hardened-blikvm-control`;
- `hardened-blikvm-server`; and
- `hardened-blikvm-update`.

All users must have distinct nonzero UIDs, distinct nonzero primary GIDs,
`/nonexistent` home, `/usr/sbin/nologin` shell, and membership in the shared
group. The shared GID must differ from every primary GID. The selected IDs must
be absent from the pre-provision account databases and must not alias root,
KVMD, Nginx, SSH, or another existing identity. The currently unused update
identity has no process, credential, writable path, or capability in this gate.

## Dedicated KVMD credential

The only KVMD username is fixed to `hardened-blikvm-broker`; it must be absent
from the original htpasswd. A new password is generated on the device only
after a nonblocking kernel `getrandom` readiness check. Its bytes never enter
host stdin/stdout, argv, environment, logs, receipts, Project Memory, source,
or Git.

The password reaches `kvmd-htpasswd set -i -q` only through a root-owned
mode-0400 file. The resulting account must authenticate a read-only `/info`
request over the existing KVMD Unix socket. Removing exactly one fixed account
line from the modified htpasswd must reproduce the byte-identical backup; this
is the rollback predicate even if a crash occurs before a modified-file hash is
recorded.

The final fixed sources are:

- `/etc/hardened-blikvm/kvmd-user`;
- `/etc/hardened-blikvm/kvmd-password`.

The directory is root-owned mode 0700. Both regular, non-symlink, single-link
files are root-owned mode 0400. They are built in one same-filesystem hidden
staging directory, synchronized, and atomically renamed into place. The broker
Unix identity must not be able to read either persistent source; PID 1 will
later expose private runtime copies through the already accepted
`LoadCredential` unit contract.

## Prepare, apply, and commit

Preflight first reuses the exact retained-package validator. It additionally
requires the six identities, dedicated KVMD account, configuration and staging
roots, account-management temporary files, provisioning gates, listeners, and
active/enabled product services to be absent. KVMD services and the read-write
rootfs remain active. `vipw`, `vigr`, user/group mutation tools, and
`systemd-sysusers` must be idle.

Prepare performs no production mutation. In one root-only persistent rollback
root it:

1. copies passwd, group, shadow, gshadow, and KVMD htpasswd with metadata;
2. installs the exact fixed sysusers source into a synthetic root;
3. runs `systemd-sysusers` twice there to pin and reproduce the chosen numeric
   IDs;
4. proves that removing only the six intended names from each synthetic
   database reproduces its backup and records before/after hashes;
5. generates the credential after the CSPRNG readiness check;
6. writes immutable rollback state plus scripts;
7. installs and synchronizes a marker-bound early-boot rollback unit; and
8. starts an independent timed watchdog.

Apply begins only after both rollback paths are active. It runs the pinned
sysusers transition, requires all four account-database hashes to match the
synthetic prediction, adds and verifies the KVMD account, then atomically
publishes the two credential sources. A synchronized allowlisted phase marker
follows each completed mutation, but rollback derives safety from content and
metadata rather than trusting the marker alone.

Before the first live apply, `stage_device_runtime_provision_prepare.py` must
execute prepare and the exact rollback script without calling the live
sysusers, htpasswd, config, apply, or commit paths. Its final state is the same
retained inert package with no service identity or credential. This separately
accepts numeric-ID prediction, auxiliary-file modeling, CSPRNG generation,
boot-unit installation, watchdog operation, and complete sensitive-stage
cleanup on the real device.

Before commit, the gate repeats identity, htpasswd, credential, package, unit,
listener, dpkg-audit, Nginx, KVMD, and rootfs checks. All four product units
must remain loaded, disabled, and inactive. Commit stops the timed watchdog,
removes the boot link/unit, and removes the sensitive rollback root only after
the retained provisioned state is fully revalidated.

## Rollback and crash behavior

Rollback first stops only the four fixed product units if any became active; it
does not stop or restart KVMD. Before changing anything, it validates every
backup, marker, script, stage, target, and all current database states.

Each account database is accepted only as its exact before hash or exact
predicted-after hash. Mixed before/after state after a crash is recoverable;
each predicted-after file is restored atomically from its matching backup.
Any third hash is a hard stop, because overwriting an unknown concurrent account
change would be less safe than retaining bounded recovery evidence.

`recover_device_runtime_provision.py` may be used only when discovery finds one
marker-bound rollback root with a complete state file, exact before/after or
mixed account databases, exact before/after htpasswd, an allowed config phase,
the retained package, inactive product units, active KVMD, and a read-write
rootfs. It replaces only that root's hash-verified rollback script, executes it,
and requires the independent retained-package-only final state. It has no apply
or commit path.

Htpasswd is left unchanged when already equal to the backup. Otherwise it must
contain exactly one dedicated account line and filtering only that line must
equal the backup before atomic restoration. Configuration and credential files
are removed only when marker, closed-tree, content, mode, owner, and link-count
checks match. Temporary restore files are similarly fixed-name and bounded.

No broad recursive deletion, caller-selected path, plaintext receipt, physical
Power/Reset, reboot, shutdown, package purge, Nginx mutation, or update action
is permitted.

## Acceptance and deferred work

Host acceptance requires exact parser/state-machine tests, generated POSIX
shell validation, BasedPyright, source/systemd boundaries, and the complete
project matrix. Native acceptance requires a private mode-0600 provisioning
receipt plus an independent read-only receipt proving the exact retained
package, six identities, dedicated KVMD authentication, closed root-only
credential tree, inactive/disabled units, free listeners, clean dpkg state,
active KVMD, and no rollback artifacts.

Service activation, loopback runtime acceptance, Nginx/TLS integration,
resource/privacy soak, reboot persistence, production update trust/recovery,
persistent diagnostic history, physical Power/Reset, and release remain later
gates.
