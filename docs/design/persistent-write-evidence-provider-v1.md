# Persistent write evidence provider v1

Status: design. Commit, retention and any device write remain disabled. This
document defines how the privileged native commit path converts sealed receipt
custody and pinned producers into the nine `GateEvidence` records required by
`persistent_write_contract.require_commit`. It authorizes no device action.

## Decision

The provider is a pure composition of three existing boundaries:

- `persistent_write_evidence_store` owns immutable, hash-addressed custody;
- per-gate validators own receipt semantics and producer pinning;
- `persistent_write_evidence_compatibility` owns fresh/historical partitioning
  and component equality.

The provider runs inside the common controller lock and supplies
`persistent_write_commit.commit(root, provider, retention_approved=True)`. The
operator anchor is the seal digest printed by the native `evidence` action; the
explicit `--approve-retention` flag is the only retention approval. No
caller-supplied receipt or producer hash is ever proof.

## Gate freshness

Five gates must bind to the current transaction:

- `host`, `native_upload`, `gui_failure_retry`, `gui_cancel`, `gui_logout`.

Four historical gates are accepted only through compatibility:

- `native_activation` and `gui_happy_path` must prove forward confirmation that
  ends exactly at the current baseline;
- `boot_restore` and `disable_drain` must share the current baseline from a
  different transaction.

A historical gate whose source archive is absent, unreadable, or lacks a
required component fails closed. Historical receipts are never relabeled as
current.

## Producer pins

Pins live in a generated, committed module `persistent_write_evidence_pins.py`:

- `CURRENT_PINS`: sha256 of reviewed producer files at the shipped revision
  (rehearsal runner, browser sources, probe sources, upload/host producers);
- `HISTORICAL_PINS`: per historical gate, the reviewed commit and the sha256 of
  each producer read from that commit (`git show <commit>:<path>`), used only by
  that gate.

`tools/generate_persistent_write_evidence_pins.py` rewrites the module; a drift
test recomputes every entry from the working tree or the named commit and fails
on any difference. Receipts, arguments, environment or store contents never
supply pins.

## Provider pipeline

`read_verified_evidence(root, binding, *, expected_seal, pins=PINNED) -> tuple[GateEvidence, ...]`:

1. `binding.validate()`; `expected_seal` is 64-hex.
2. Open `Store(binding)`; parse `seal.json` with duplicate-key rejection, exact
   schema, binding equality and a complete `REQUIRED_GATES` map of 64-hex
   digests.
3. `Store.verify_seal(gate_map, expected_seal)` durably re-reads the seal,
   binding and every referenced object.
4. For each gate in sorted order, `Store.get(digest)` returns the original
   bytes; the registered validator receives `(binding, raw, pins)` and either
   accepts or raises. A gate without a registered validator fails closed.
5. `GateEvidence(gate, binding, digest, True)` is built only from accepted
   bytes.

Historical gates additionally pass `compatibility.check` using the component
view of the source archive and of the current plan.

## Component view

`component_view(plan) -> dict[str, str]` is derived from the plan's stable
fields only; no receipt or caller input contributes:

| component | source |
| --- | --- |
| updaterPackage | `plan["files"]["new.deb"]` |
| applicationPayload | `plan["files"]["bindings.json"]` (the signed payload binding manifest) |
| controller | `plan["files"]["controller.py"]` |
| recoveryPolicy | joined digests of the stable guard/lifecycle helpers from `plan["files"]` |
| testHarness | producer pins used by that gate's validator |
| trustRoot | `plan["before"]["runtime"]["trust"]` |
| unitContracts | joined `plan["before"]["persistentDropinHashes"]` (permanent wired overrides) |
| proxyExecutable | `plan["before"]["nginxUnit"]` (pinned hardened frontend unit contract) |
| proxyConfiguration | `plan["before"]["runtime"]["nginxConfig"]` |

Deviations and boundaries:

- The plans do not record the frontend executable digest. `proxyExecutable`
  therefore uses the exact hardened Nginx unit contract; it must not be
  described as binary provenance. A future producer that records the binary
  digest replaces this mapping in a new plan schema.
- The archived plan reader accepts legacy plans without newer keys such as
  `disableStarted`; it never calls the current controller loader on an archive
  and reads only the fields named above.
- A missing entry fails the historical gate closed; components are never
  synthesized or defaulted. `persistent_write_component_view.py` implements
  the mapping and is tested against a synthetic plan fixture before any device
  receipt is used.

## Cycle shape

Recovery prerequisites and the commit transaction are deliberately separate:

1. **Prerequisite transaction** under the shipped controller: prepare the wired
   policy, run the held-upload drain and the software-reboot early-boot gates,
   then restore and clean up. Its receipts and archives are produced by the
   current controller revision.
2. **Commit transaction** under the same revision: prepare the wired policy,
   run the five fresh gates, stage all nine receipts into the evidence store,
   seal, and commit with explicit approval.
3. `native_activation` and `gui_happy_path` are taken from the accepted
   application update that established the current baseline; their component
   set is `COMMON` only, so a controller change between that update and the
   commit transaction does not invalidate them.
4. `boot_restore` and `disable_drain` come from the prerequisite transaction
   archives; their controller/recoveryPolicy components must equal the commit
   transaction's, which the separate-transaction shape guarantees.

## Validators

| gate | validator |
| --- | --- |
| native_activation, gui_happy_path | generalized activation-receipt parser over pinned profiles (existing seq8→seq9 plus seq9→seq10; old profiles and tests unchanged) |
| boot_restore, disable_drain | existing recovery-receipt parser with historical probe pins |
| gui_failure_retry, gui_cancel, gui_logout | one negative-browser-receipt parser with three assertion sets, current producers and binding |
| native_upload | upload-worker receipt bound to the current guard root |
| host | bounded host-gate receipt from the reviewed `capture_persistent_write_host_gate.py` producer; v1 scope is the persistent-write controller suite, with server authorization/length/quota/cancellation families documented as a later extension |

## Failure rules

- Missing validator, missing component, stale or mismatched binding, seal
  mismatch, or missing historical archive fails closed with a typed error.
- The provider never publishes, revokes or repairs a decision and never closes
  admission. Publication stays with the commit backend under the same lock.
- No retention or device action follows from this design. The native `commit`
  action requires both `--evidence-seal` and `--approve-retention`.

## Tests

- pins drift test (regenerated module equals the committed one);
- provider tests against a real temporary store: seal mismatch, missing
  validator, binding mismatch, gate dispatch, fail-closed order;
- per-gate validator tests with pinned producers, tampered fields and
  cross-transaction rejection;
- historical compatibility tests with synthetic archives and the component
  view fixture.
