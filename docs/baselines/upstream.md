# Upstream source baseline

Verified on 2026-08-30.

| Source | Revision | Role |
| --- | --- | --- |
| `blikvm/blikvm` | `3e34c04a1dfa84636050585d12d5e585098258ea`, tag `v2.2.3-alpha` | Current BliKVM hardware and application reference |
| `woffko/Bli-PiKVM` | `6966a969f46f80172c5a1d1e0aa48be812625ac2` | PiKVM/KVMD-on-H616 procedure and MSD compatibility reference |
| Hardened NanoKVM checkout | `387793ab944437bbedc534d2ba231dd88d1fd2ca` | Reusable Rust/React/security/release source baseline |

The inspected field image contained an older vendor checkout at
`26fa184b9305ea256ece8ed7aaec3b9ede29ab1f` and KVMD `3.291`. It is evidence
for the existing hardware integration only. It is not a production source or
update baseline.

Before importing code, record the exact file-level license and provenance.
Never silently replace one pinned revision with a newer working tree.
