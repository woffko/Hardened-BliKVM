# Reversible H616 systemd and dedicated-account checkpoint

On 2026-08-30, the commit-bound development bundle
`420c6ba4ed4baf9127184b489b26701e0144f6891c549631dfd58a82c2c95118`
passed a reversible systemd and dedicated KVMD account gate on the designated
test-only H616 device. The bundle source commit was
`343976b5c22e0b062aab6e19f22ca92d4beb7a35`.

Before changing authentication state, the runner copied the complete KVMD
htpasswd file with its metadata and recorded its SHA-256. It generated a
dedicated random credential without placing it in argv, environment, logs, or
receipts, added the account through the native `kvmd-htpasswd set -i` command,
and armed a ten-minute systemd rollback watchdog. An unknown concurrent
htpasswd change would make rollback fail closed instead of overwriting it.

The exact broker and server unit templates were installed only in the runtime
unit directory. Test-only drop-ins substituted existing non-root identities,
temporary paths, and an alternate loopback port; the production sandbox
directives remained active.

Verified results:

- the dedicated KVMD account authenticated directly over the internal Unix
  socket;
- the broker received the account through systemd `LoadCredential`, ran in a
  private network namespace, had no capabilities, and created a mode-0660
  broker socket;
- the server ran without KVMD credential environment or files, had no
  capabilities, and remained restricted to host loopback;
- first-account setup, login, and twenty authenticated sanitized inventory
  requests returned HTTP 200;
- the original KVMD and KVMD Nginx services stayed active and were never
  restarted;
- both test units stopped, the dedicated account disappeared, and the KVMD
  htpasswd SHA-256 returned exactly to its pre-test value;
- runtime units, watchdog, account, port, and marker-protected `/opt`, `/tmp`,
  and `/run` staging were absent in the final check;
- the root filesystem remained in its pre-existing read-write state and no
  root-filesystem mode helper was invoked.

No HID event, ATX action, GPIO operation, media mutation, update, reboot,
physical power, or physical reset action was invoked. This checkpoint does not
accept a permanent installation, production TLS proxy, reboot persistence,
recovery, or physical hardware behavior.
