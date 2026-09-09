import assert from "node:assert/strict";
import test from "node:test";
import {
  captureApplicationResponses,
  responseCaptureKey as key,
  responseRoutes as routes,
} from "./playwright/application_response_capture.mjs";

function fixture(action) {
  const oldWindow = globalThis.window;
  const oldXhr = globalThis.XMLHttpRequest;
  class Xhr extends EventTarget {
    open(...args) {
      this.originalArguments = args;
      return "unchanged-open-result";
    }
    status = 200;
    responseType = "";
    responseText = '{"code":0}';
  }
  globalThis.window = { location: { href: "https://fixture.invalid/" } };
  globalThis.XMLHttpRequest = Xhr;
  try {
    captureApplicationResponses({ key, routes });
    action(Xhr, window[key]);
  } finally {
    if (oldWindow === undefined) delete globalThis.window;
    else globalThis.window = oldWindow;
    if (oldXhr === undefined) delete globalThis.XMLHttpRequest;
    else globalThis.XMLHttpRequest = oldXhr;
  }
}

test("observer preserves open arguments and captures only completed update responses", () =>
  fixture((Xhr, captured) => {
    const xhr = new Xhr();
    const args = ["POST", routes[0], true];
    assert.equal(xhr.open(...args), "unchanged-open-result");
    assert.deepEqual(xhr.originalArguments, args);
    assert.equal(captured[routes[0]].finished, false);
    xhr.dispatchEvent(new Event("loadend"));
    assert.deepEqual(captured[routes[0]], {
      finished: true,
      status: 200,
      body: '{"code":0}',
      error: null,
    });
  }));

test("login and read-only responses are not captured", () =>
  fixture((Xhr, captured) => {
    for (const [method, route] of [
      ["POST", "/api/auth/login"],
      ["GET", routes[0]],
    ]) {
      const xhr = new Xhr();
      xhr.open(method, route);
      xhr.dispatchEvent(new Event("loadend"));
    }
    assert.equal(Object.keys(captured).length, 0);
  }));

test("oversize and duplicate responses fail closed", () =>
  fixture((Xhr, captured) => {
    const first = new Xhr();
    first.open("POST", routes[0]);
    first.responseText = "x".repeat(65537);
    first.dispatchEvent(new Event("loadend"));
    assert.equal(captured[routes[0]].error, "invalid-response-size");
    assert.equal(captured[routes[0]].body, null);
    const second = new Xhr();
    second.open("POST", routes[0]);
    assert.equal(captured[routes[0]].error, "duplicate-response");
  }));
