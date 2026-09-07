#!/usr/bin/env node
// A deliberately small HTTPS reverse proxy used only by the real Demo E2E project.
// Production ingress is Caddy; this process exists so the browser suite exercises
// Secure cookies, exact origins, forwarded client-IP handling, and streaming responses.
import { request as httpRequest } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { readFileSync } from "node:fs";
import { isIP } from "node:net";

const HOP_BY_HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

function clientAddress(socket) {
  const address = socket.remoteAddress;
  if (!address || isIP(address) === 0) return "127.0.0.1";
  return address.startsWith("::ffff:")
    ? address.slice("::ffff:".length)
    : address;
}

function sendProxyError(response, statusCode, message) {
  if (response.headersSent) {
    response.destroy();
    return;
  }

  const body = JSON.stringify({ error: message });
  response.writeHead(statusCode, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store",
    connection: "close",
  });
  response.end(body);
}

function forwardedRequestHeaders(incoming, socket, backendHost, backendPort) {
  const headers = {};
  for (const [name, value] of Object.entries(incoming.headers)) {
    const lowerName = name.toLowerCase();
    if (
      HOP_BY_HOP_HEADERS.has(lowerName) ||
      lowerName === "host" ||
      lowerName === "x-bhtune-client-ip" ||
      lowerName === "forwarded" ||
      lowerName === "x-forwarded-for" ||
      lowerName === "x-real-ip"
    ) {
      continue;
    }
    if (value !== undefined) headers[name] = value;
  }

  headers.host = `${backendHost}:${backendPort}`;
  // The backend trusts this header only from this proxy's configured peer. Replacing
  // it here makes an inbound spoofed header impossible to pass through the harness.
  headers["x-bhtune-client-ip"] = clientAddress(socket);
  return headers;
}

/**
 * Start the loopback HTTPS proxy and resolve after its listening socket is ready.
 *
 * The returned server is intentionally a normal Node server so the launcher can close
 * it before stopping the backend and can force-close a long-lived SSE connection during
 * Playwright teardown.
 */
export async function startHttpsProxy({
  listenHost,
  listenPort,
  backendHost,
  backendPort,
  keyPath,
  certificatePath,
  backendTimeoutMs = 15_000,
}) {
  const server = createHttpsServer(
    {
      key: readFileSync(keyPath),
      cert: readFileSync(certificatePath),
    },
    (incoming, response) => {
      const upstream = httpRequest(
        {
          hostname: backendHost,
          port: backendPort,
          method: incoming.method,
          path: incoming.url || "/",
          headers: forwardedRequestHeaders(
            incoming,
            incoming.socket,
            backendHost,
            backendPort,
          ),
          agent: false,
        },
        (upstreamResponse) => {
          const responseHeaders = {};
          for (const [name, value] of Object.entries(
            upstreamResponse.headers,
          )) {
            if (
              !HOP_BY_HOP_HEADERS.has(name.toLowerCase()) &&
              value !== undefined
            ) {
              responseHeaders[name] = value;
            }
          }
          response.writeHead(
            upstreamResponse.statusCode ?? 502,
            responseHeaders,
          );
          upstreamResponse.pipe(response);
        },
      );

      let responseStarted = false;
      upstream.once("response", () => {
        responseStarted = true;
      });
      upstream.setTimeout(backendTimeoutMs, () => {
        upstream.destroy(new Error("backend response timed out"));
        if (!responseStarted) {
          sendProxyError(
            response,
            504,
            "The Demo backend did not respond before the proxy timeout.",
          );
        } else {
          response.destroy();
        }
      });
      upstream.once("error", (error) => {
        if (!responseStarted) {
          console.error(`e2e: Demo proxy backend error: ${error.message}`);
          sendProxyError(
            response,
            502,
            "The Demo backend is unavailable through the test proxy.",
          );
        } else {
          response.destroy();
        }
      });
      incoming.once("aborted", () => upstream.destroy());
      response.once("close", () => {
        if (!response.writableFinished) upstream.destroy();
      });
      incoming.pipe(upstream);
    },
  );

  await new Promise((resolve, reject) => {
    const onError = (error) => {
      server.off("listening", onListening);
      reject(error);
    };
    const onListening = () => {
      server.off("error", onError);
      resolve();
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(listenPort, listenHost);
  });

  return server;
}

export async function closeHttpsProxy(server) {
  // A browser may still have an EventSource open when Playwright tears down the
  // webServer. Close those sockets explicitly so teardown never waits for the
  // Demo stream's normal 45-second lifetime.
  server.closeAllConnections?.();
  await new Promise((resolve, reject) => {
    server.close((error) => {
      if (error) reject(error);
      else resolve();
    });
  });
}
