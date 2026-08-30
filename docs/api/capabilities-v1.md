# Capability manifest v1

`GET /api/capabilities` returns the normal API envelope and a versioned
manifest. Availability and evidence are deliberately separate:

- `unavailable`, `read_only`, `read_write` describe what the running BLi-KVM
  backend exposes now;
- `none`, `user_reported`, `source_verified`, `runtime_verified`, and
  `physical_verified` describe the strongest supporting evidence.

Evidence never authorizes a control. A status may intentionally combine
`availability: unavailable` with `evidence: runtime_verified`: the reference
PiKVM stack reported the hardware online, while this Rust backend still exposes
no matching endpoint. A frontend must fail closed on `availability` and may use
`evidence` only for explanatory inventory. Production release gates require
physical evidence for hardware-mutating features.

Transport, mode, and action lists attached to an unavailable status describe
the observed porting target. They are not actionable until the parent status
becomes available.

The contract is defined by `bli-kvm-core::CapabilityManifest`. The built-in
H616 manifest contains no endpoint, credential, SSH path, or private network
information.
