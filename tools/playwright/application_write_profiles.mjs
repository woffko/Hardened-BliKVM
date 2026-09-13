import assert from "node:assert/strict";

const profiles = Object.freeze({
  "seq10-to11": Object.freeze({
    source: "seq10",
    target: "seq11",
    bundleBytes: 8724480,
    bundleSha256:
      "e113e4bc1d9091d294ea499e775a03e685f51f75f362c3cbb08c0fbaf005ec40",
    payloadBytes: "8683976",
    disposition: "published",
    stagedMessage: "Archive verified and staged",
  }),
  "seq9-to10": Object.freeze({
    source: "seq9",
    target: "seq10",
    bundleBytes: 8724480,
    bundleSha256:
      "85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f",
    payloadBytes: "8683976",
    disposition: "published",
    stagedMessage: "Archive verified and staged",
  }),
  "seq10-retry": Object.freeze({
    source: "seq10",
    target: "seq10",
    bundleBytes: 8724480,
    bundleSha256:
      "85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f",
    payloadBytes: "8683976",
    disposition: "already_present",
    stagedMessage: "This verified archive is already staged",
  }),
  "seq9-retry": Object.freeze({
    source: "seq9",
    target: "seq9",
    bundleBytes: 8724480,
    bundleSha256:
      "8f858ec697928f39bcfc6a7242aa2d1fdbb061b8cf9232048d9468d03dff247f",
    disposition: "already_present",
    stagedMessage: "This verified archive is already staged",
  }),
  "seq7-to8": Object.freeze({
    source: "seq7",
    target: "seq8",
    bundleBytes: 8724480,
    bundleSha256:
      "d2acf270fa2aa9cb924c362066ece50f4b0fbd4c4c8b2975648dab5ac0b4f849",
    disposition: "already_present",
    stagedMessage: "This verified archive is already staged",
  }),
  "seq8-to9": Object.freeze({
    source: "seq8",
    target: "seq9",
    bundleBytes: 8724480,
    bundleSha256:
      "8f858ec697928f39bcfc6a7242aa2d1fdbb061b8cf9232048d9468d03dff247f",
    disposition: "published",
    stagedMessage: "Archive verified and staged",
  }),
});

export function writeProfile(name = "seq7-to8") {
  assert.equal(typeof name, "string");
  assert.ok(Object.hasOwn(profiles, name), "unknown write acceptance profile");
  return profiles[name];
}
