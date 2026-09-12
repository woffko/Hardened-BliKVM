import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import {
  watchUploadTermination,
  validateUploadTermination,
} from "./playwright/upload_termination.mjs";

const route = "/api/application-update/upload";
const request = () => ({
  method: () => "POST",
  url: () => "https://fixture" + route,
  failure: () => ({ errorText: "net::ERR_ABORTED" }),
});
for (const kind of ["response", "failed"]) {
  const page = new EventEmitter();
  const watch = watchUploadTermination(page, route);
  const original = request();
  page.emit("request", original);
  watch.markLogoutRequested();
  // A different request must not satisfy this upload's termination.
  page.emit("requestfailed", request());
  if (kind === "response")
    page.emit("response", { request: () => original, status: () => 401 });
  else page.emit("requestfailed", original);
  const result = await watch.done;
  assert.equal(result.kind, kind);
  validateUploadTermination(result);
  watch.dispose();
  assert.equal(page.eventNames().length, 0);
}
for (const value of [
  { kind: "response", status: 200, afterLogoutRequested: true },
  { kind: "failed", aborted: true, afterLogoutRequested: false },
  { kind: "failed", aborted: false, afterLogoutRequested: true },
])
  assert.throws(() => validateUploadTermination(value));
const page = new EventEmitter();
const watch = watchUploadTermination(page, route);
const original = request();
page.emit("request", original);
page.emit("requestfailed", original);
assert.throws(() => watch.markLogoutRequested());
const endedBeforeLogout = await watch.done;
assert.throws(() => validateUploadTermination(endedBeforeLogout));
watch.dispose();
const silentPage = new EventEmitter();
const silent = watchUploadTermination(silentPage, route, 5);
silentPage.emit("request", request());
silent.markLogoutRequested();
await assert.rejects(silent.done);
silent.dispose();
