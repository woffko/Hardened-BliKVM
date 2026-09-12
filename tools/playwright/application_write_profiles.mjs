import assert from "node:assert/strict";

const profiles = Object.freeze({
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
