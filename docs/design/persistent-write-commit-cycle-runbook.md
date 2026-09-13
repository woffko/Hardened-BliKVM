# Permanent-admission commit cycle runbook

This runbook describes the reviewed device cycle that retains permanent GUI
update admission for a signed baseline. It is a procedure, not an approval:
each device-mutating step requires explicit operator confirmation, and no
target Power/Reset action is ever used.

## Starting state

- Confirmed signed baseline with closed write admission and no retained guard
  (for the seq10 baseline: seq10, floor 10, generation 46, previous seq9).
- The accepted update that established the baseline, with its archived
  rehearsal root available for the historical activation gates.
- Reviewed old/new updater packages and the exact signed application archive.
- Enrolled SSH and GUI test credentials supplied only on stdin (for example
  through `project-memory-exec`); the endpoint, base URL and account come from
  the enrolled test asset or local configuration and never from a public file.

## Step 1 — prerequisite drain transaction

```
tools/run_persistent_write_rehearsal.py pending \
  --application-profile seq10 \
  --old-package <reviewed old updater package> \
  --new-package <reviewed new updater package> \
  --endpoint <ssh endpoint> \
  --bundle <signed application archive> \
  --output evidence/private/<new pending receipt>
```

This produces the `disable_drain` prerequisite under the current controller.
It must end restored, cleaned and closed with the baseline unchanged.

## Step 2 — prerequisite reboot transaction

```
tools/run_persistent_write_rehearsal.py boot --reboot \
  --application-profile seq10 \
  --old-package <reviewed old updater package> \
  --new-package <reviewed new updater package> \
  --endpoint <ssh endpoint> \
  --output evidence/private/<new boot receipt>
```

This produces the `boot_restore` prerequisite: one software reboot of the
BliKVM, early-boot guard recovery before serving, restore, cleanup, and a
changed boot identity with the baseline unchanged. Steps 1 and 2 run separately
so their controller and recovery-policy components match the commit
transaction.

## Step 3 — commit transaction

```
tools/run_persistent_write_commit_cycle.py \
  --endpoint <ssh endpoint> \
  --old-package <reviewed old updater package> \
  --new-package <reviewed new updater package> \
  --bundle <signed application archive> \
  --base-url <https test endpoint> \
  --username <test account> \
  --application-profile seq10 \
  --activation-receipt <accepted activation receipt> \
  --boot-receipt <step 2 receipt> \
  --pending-receipt <step 1 receipt> \
  --output evidence/private/<new commit cycle receipt>
```

The orchestrator prepares and wires the transaction, then inside one guard
window runs the host gate, the in-window native upload gate, the in-transaction
negative GUI gate, stages and seals all nine receipts in the transaction
evidence store, and submits the native commit with the explicit retention
approval. Fresh gates bind to the current transaction; `native_activation` and
`gui_happy_path` reuse the accepted activation receipt through compatibility;
`boot_restore` and `disable_drain` come from the prerequisite transactions.

Any failure before publication restores admission and cleans the guard. A
failure after publication never closes the committed policy and leaves state
for inspection.

## Step 4 — retained reboot survival

```
tools/run_persistent_write_retained_reboot.py \
  --endpoint <ssh endpoint> \
  --guard-root <committed guard root from step 3> \
  --output evidence/private/<new retained reboot receipt>
```

The driver verifies the retained policy before the reboot, requests exactly one
software reboot, and polls the read-only verified snapshot until the boot
identity changes and retention is still confirmed.

## Step 5 — record and publish

Record the real gate outcomes in `docs/baselines/` and push the source changes.
Private receipts, endpoints, accounts and credentials remain outside Git.

## Decision

The agreed end state retains the committed policy; the permanent disable
workflow stays covered by host tests and is not executed as part of this cycle.
