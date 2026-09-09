import assert from "node:assert/strict";

export const responseCaptureKey = "__bliAcceptanceUpdateResponses";
export const responseRoutes = [
  "/api/application-update/upload",
  "/api/application-update/activate",
];

// Executed by addInitScript before application code. Observe loadend only:
// no request body, headers, transport, or response bytes are replaced.
export function captureApplicationResponses({ key, routes }) {
  const responses = Object.create(null);
  Object.defineProperty(window, key, { value: responses });
  const original = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (...args) {
    const result = original.apply(this, args);
    let route;
    try {
      route = new URL(String(args[1]), window.location.href).pathname;
    } catch {
      return result;
    }
    if (String(args[0]).toUpperCase() !== "POST" || !routes.includes(route))
      return result;
    const entry = { finished: false, status: null, body: null, error: null };
    if (responses[route]) {
      responses[route].error = "duplicate-response";
      return result;
    }
    responses[route] = entry;
    this.addEventListener(
      "loadend",
      () => {
        try {
          entry.status = this.status;
          const body =
            this.responseType === "json"
              ? JSON.stringify(this.response)
              : this.responseText;
          if (typeof body !== "string" || body.length > 65536)
            entry.error = "invalid-response-size";
          else entry.body = body;
        } catch {
          entry.error = "unreadable-response";
        }
        entry.finished = true;
      },
      { once: true },
    );
    return result;
  };
}

export async function installResponseCapture(context) {
  await context.addInitScript(captureApplicationResponses, {
    key: responseCaptureKey,
    routes: responseRoutes,
  });
}

export async function readCapturedResponse(page, route, httpStatus) {
  assert.ok(responseRoutes.includes(route));
  await page.waitForFunction(
    ({ key, route }) => window[key]?.[route]?.finished === true,
    { key: responseCaptureKey, route },
    { timeout: 45000 },
  );
  const value = await page.evaluate(({ key, route }) => window[key][route], {
    key: responseCaptureKey,
    route,
  });
  assert.equal(value.error, null);
  assert.equal(value.status, httpStatus);
  assert.equal(typeof value.body, "string");
  assert.ok(value.body.length <= 65536);
  return JSON.parse(value.body);
}
