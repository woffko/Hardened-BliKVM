// Shared local harness for the application-level GUI probes: serve the built
// web/dist and answer the API routes the desktop page and the restricted
// controls need. It is a test support module, never a shipped artifact.

import fs from "node:fs";
import http from "node:http";
import path from "node:path";

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

export function defaultManifest(overrides = {}) {
  const status = (availability) => ({
    availability,
    evidence: "source_verified",
  });
  return {
    schemaVersion: 1,
    product: {
      name: "Hardened BliKVM",
      shortName: "BLi-KVM",
      tagline: "fixture",
    },
    platform: {
      id: "bli-h616-pikvm",
      board: "BliKVM v4 H616",
      architecture: "aarch64",
      operatingSystem: "Armbian",
    },
    video: {
      status: status("read_only"),
      transports: ["mjpeg"],
      preferredTransport: "mjpeg",
      maxWidth: null,
      maxHeight: null,
      audio: status("unavailable"),
      settings: ["fps", "quality"],
    },
    hid: {
      status: status("read_write"),
      modes: ["keyboard", "absolute_mouse", "relative_mouse"],
    },
    power: {
      status: status("read_write"),
      actions: ["power_short", "power_long", "reset"],
      ledState: status("read_only"),
    },
    virtualMedia: {
      status: status("read_write"),
      modes: ["cd_rom", "removable_disk"],
      resumableUpload: status("unavailable"),
      remoteDownload: status("unavailable"),
    },
    board: { features: {} },
    system: {
      status: status("read_only"),
      readOnlyRootfs: true,
      maintenanceWriteMode: false,
      actions: status("unavailable"),
      signedApplicationUpdates: status("unavailable"),
      signedSystemUpdates: status("unavailable"),
      automaticRollback: status("unavailable"),
    },
    network: {
      status: status("read_only"),
      dns: status("read_only"),
      ipv6: status("read_only"),
      wol: status("unavailable"),
    },
    ...overrides,
  };
}

/**
 * Serve `web/dist` and the routes the probes need. Records every API request as
 * `METHOD /path` in `requests` and returns the base URL.
 */
// The headers nginx sends with every page, read from the shipped
// configuration so the contracts run under the same policy.
export function shippedSecurityHeaders() {
  const conf = fs.readFileSync(
    path.resolve("packaging/nginx/hardened-blikvm.conf"),
    "utf8",
  );
  const headers = {};
  for (const match of conf.matchAll(
    /add_header (Content-Security-Policy|X-Content-Type-Options|Referrer-Policy) "([^"]+)"/g,
  ))
    headers[match[1]] = match[2];
  return headers;
}

export async function startApp({
  dist,
  manifest = defaultManifest(),
  hidLeaseApplied = true,
} = {}) {
  const root = dist ?? path.resolve("web/dist");
  const requests = [];
  const requestDetails = [];
  const securityHeaders = shippedSecurityHeaders();
  const server = http.createServer((request, response) => {
    const route = new URL(request.url, "http://127.0.0.1").pathname;
    const json = (value, status = 200, headers = {}) => {
      response.writeHead(status, {
        "Content-Type": "application/json",
        ...headers,
      });
      response.end(JSON.stringify(value));
    };
    if (route.startsWith("/api/")) {
      requests.push(`${request.method} ${route}`);
      const detail = { method: request.method, route, body: null };
      requestDetails.push(detail);
      const chunks = [];
      request.on("data", (chunk) => chunks.push(chunk));
      request.on("end", () => {
        if (chunks.length === 0) return;
        const body = Buffer.concat(chunks).toString("utf8");
        try {
          detail.body = JSON.parse(body);
        } catch {
          detail.body = body;
        }
      });
      if (route === "/api/auth/setup" && request.method === "GET") {
        json({ code: 0, data: { required: false } });
        return;
      }
      if (route === "/api/auth/login" && request.method === "POST") {
        json({ code: 0, data: { csrfToken: "fixture-csrf" } }, 200, {
          "Set-Cookie": "session=fixture; Path=/; HttpOnly",
        });
        return;
      }
      if (route === "/api/capabilities") {
        json({ code: 0, data: manifest });
        return;
      }
      if (route === "/api/inventory") {
        json({ code: 0, data: { schemaVersion: 3 } });
        return;
      }
      if (route === "/api/application/current-version") {
        json({ code: 0, data: { current: "1.0.0" } });
        return;
      }
      if (route === "/api/hid/focus" && request.method === "GET") {
        json({ code: 0, data: { challenge: "a".repeat(32) } });
        return;
      }
      if (route === "/api/hid/focus" && request.method === "POST") {
        json({ code: 0, data: { attested: true } });
        return;
      }
      if (route === "/api/hid/lease" && request.method === "POST") {
        json(
          hidLeaseApplied
            ? {
                code: 0,
                data: {
                  applied: true,
                  leaseId: "b".repeat(32),
                  ttlSeconds: 20,
                },
              }
            : {
                code: 0,
                data: { applied: false, rejection: "backend_unavailable" },
              },
        );
        return;
      }
      if (route === "/api/application-power" && request.method === "POST") {
        json({ code: 0, data: { applied: true } });
        return;
      }
      if (route === "/api/virtual-media/catalog") {
        json({
          code: 0,
          data: {
            images: [
              {
                imageId: 1,
                imageName: "kubuntu.img",
                size: 1024,
                cdromAllowed: false,
                removableAllowed: true,
                writableAllowed: false,
              },
            ],
          },
        });
        return;
      }
      if (route === "/api/virtual-media" && request.method === "POST") {
        json({ code: 0, data: { applied: true } });
        return;
      }
      if (route === "/api/stream/mjpeg") {
        response.writeHead(503, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ code: 1, msg: "no stream" }));
        return;
      }
      json({ code: 1, msg: "not found" }, 404);
      return;
    }
    const relative = route === "/" ? "index.html" : route.replace(/^\/+/, "");
    const candidate = path.join(root, relative);
    if (
      candidate.startsWith(root) &&
      fs.existsSync(candidate) &&
      fs.statSync(candidate).isFile()
    ) {
      response.writeHead(200, {
        "Content-Type":
          CONTENT_TYPES[path.extname(candidate)] ?? "application/octet-stream",
        ...securityHeaders,
      });
      response.end(fs.readFileSync(candidate));
      return;
    }
    response.writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      ...securityHeaders,
    });
    response.end(fs.readFileSync(path.join(root, "index.html")));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    server,
    requests,
    requestDetails,
    baseUrl: `http://127.0.0.1:${server.address().port}`,
  };
}
