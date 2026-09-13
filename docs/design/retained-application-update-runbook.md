# Retained application update runbook

Status: routine procedure under the committed permanent write admission. The
policy is retained on the test device; no temporary rehearsal guard is used.
Device steps still require explicit operator confirmation.

## 1. Build and verify the signed archive

Build the next signed application archive with the accepted runtime package and
the enrolled production key:

```
tools/build_production_update_bundle.py \
  --package <accepted runtime package> \
  --expected-package-sha256 <package sha> \
  --expected-source-commit <package source commit> \
  --expected-source-bundle-sha256 <package source bundle sha> \
  --expected-builder-commit <package builder commit> \
  --enrollment-request <enrolled public request> \
  --expected-enrollment-request-sha256 <request sha> \
  --key-id app-production-2026-09-05-01 \
  --version <application version> --sequence <next sequence> --minimum-system-sequence 1 \
  --output-dir /mnt/d/debug/blikvm/hardened-blikvm/<new output> \
  --acknowledge-unencrypted-host-key-risk
```

Build the same archive twice and require byte-identical bytes. Then run
`tools/verify_production_update_bundle.py` with both archives, the request, the
enrolled key id and a host Rust supervisor; it must pass without contacting the
device or executing payload code.

## 2. Record the browser profiles

- `tools/playwright/application_write_profiles.mjs`: add the target profile
  `<N>-to-<N+1>` (`disposition: published`) and the `<N+1>-retry` profile
  (`disposition: already_present`), both with the exact archive size and SHA-256.
- `tools/playwright/status_api_acceptance.mjs`: add the `<N+1>` status profile
  with slot, version and sequence. The generation is taken from the observed
  post-confirmation status; if the recorded value differs, correct it from the
  evidence before recording the baseline.
- Native parsers used by policy gates: add the matching upload and negative-GUI
  profiles when a future policy re-commit needs them.

## 3. Device acceptance

1. Run the activate browser probe with the new target profile. It performs one
   login, one upload and one activation and returns an `awaiting_health`
   receipt; the retained updater confirms the candidate automatically.
2. Poll the authoritative native status read-only until the confirmed floor
   advances and `pending` is `null`; capture the observed generation.
3. Run the closed GUI probe with the target profile for desktop/mobile
   read-only checks and Refresh.
4. Confirm the retained guard and evidence store are untouched and that the
   committed decision still matches.

## 4. Record and publish

Write `docs/baselines/persistent-write-seq<N+1>-retained-update.md` with exact
hashes, receipts, the observed generation, and boundaries, then push. Private
receipts, endpoints and credentials remain outside Git.

## Boundaries

- Upload never performs automatic slot garbage collection; new application
  slots accumulate and reclamation stays a separate reviewed operation.
- The permanent disable workflow remains host-tested only unless a separate
  cycle intentionally exercises it.
