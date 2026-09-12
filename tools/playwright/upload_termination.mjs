import assert from "node:assert/strict";

// Bind to one original request. Do not substitute a fresh authenticated probe
// for observing its termination, or count browser teardown as cancellation.
export function watchUploadTermination(page, route, timeoutMs = 30000) {
  let request = null;
  let logoutRequested = false;
  let settled = false;
  let resolve;
  let reject;
  const done = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  void done.catch(() => {}); // May fail while the caller is still clicking UI.
  const timer = setTimeout(() => {
    settled = true;
    reject(new Error("Original upload termination not observed"));
  }, timeoutMs);
  function finish(value) {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    resolve({ ...value, afterLogoutRequested: logoutRequested });
  }
  function selected(value) {
    if (
      !request &&
      value.method() === "POST" &&
      new URL(value.url()).pathname === route
    )
      request = value;
  }
  function responded(value) {
    if (request && value.request() === request)
      finish({ kind: "response", status: value.status() });
  }
  function failed(value) {
    if (request && value === request)
      finish({
        kind: "failed",
        aborted: value.failure()?.errorText === "net::ERR_ABORTED",
      });
  }
  page.on("request", selected);
  page.on("response", responded);
  page.on("requestfailed", failed);
  return {
    done,
    markLogoutRequested() {
      assert.ok(
        request && !settled,
        "Upload must still be in flight at logout",
      );
      logoutRequested = true;
    },
    dispose() {
      clearTimeout(timer);
      page.off("request", selected);
      page.off("response", responded);
      page.off("requestfailed", failed);
    },
  };
}

export function validateUploadTermination(value) {
  assert.equal(value.afterLogoutRequested, true);
  if (value.kind === "response") assert.equal(value.status, 401);
  else {
    assert.equal(value.kind, "failed");
    assert.equal(value.aborted, true);
  }
}
