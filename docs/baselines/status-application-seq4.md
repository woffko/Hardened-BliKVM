# Status-capable signed application sequence four

Source `f22615d98ec69ff61fb49def0847df36c0ebb690` supplies the status-capable
server and GUI. Independently built runtime package inputs compared equal:
`4cff4b904029e6af84e02aefe8cbe6644cb3239ad81349c3d93e914e90d21a46`.
Their builder commit is `a88189765d32e0b7e395dfdff16c420a3cef36b0`.

Two signing runs under the already enrolled current application key produced
identical version `0.1.0-dev3`, sequence-four archives of 8,591,360 bytes:
`4ae41c49d2a4dcc0cab041516e3591d89d2da4342484123467f70e19d19a7b12`.
Manifest SHA-256 is
`768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c`;
minimum system sequence remains one. Sequence three is not reused because it
already identified the interrupted-update rehearsal candidate.

Host job `a720aa0276a8444fa3ffbc8796b8a594` passed with freshly built supervisor
`9e8152800fe4cb2fb8af0997b0bae7974138b77e5a47e020561b0332729b6c7f`.
It verified full signatures/payloads, all 27 immutable files, repeat staging,
fresh initialization/status, tamper and wrong-key rejection, and unchanged
state after negative checks. Temporary host roots were removed; payload code
was not executed and the device was not contacted.

## Device transition still required

The named `seq2-to-seq4-status` profile binds the exact archive, source/target
slots and sequences into the existing guarded transaction workflow. The legacy
`seq1-to-seq2` profile remains separate. Recovery commands and checkpoint
receipts carry the profile; unknown or cross-profile continuation fails.
Staging compares every new payload file against the bound archive, not against
the old application's bytes. The original slot remains fingerprint-protected.

First rehearse candidate health timeout and return to confirmed sequence two,
then require a phase-bound browser check before normal activation. Successful
health confirmation raises the floor to four; recovery must never restore a
lower journal backup. Browser acceptance and guard cleanup are still required
after confirmation. This transition does not enable the status socket or add
its server environment: those require the next bounded IPC/API/browser gate.

Independent review led to explicit commit-phase checking on both host and
device, including rehearsal-only settlement. Recovery records its observed
floor and refuses regression after confirmation, including subsequent cleanup
checks. A confirmation that reached the manager before the auxiliary marker
was saved remains recoverable. These are local consistency checks, not a claim
of hardware-backed rollback protection against an attacker rewriting all state.
