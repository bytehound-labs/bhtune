#!/usr/bin/env node
// Starts an isolated Demo-mode backend and places a temporary HTTPS proxy in front of it.
// This is intentionally separate from start-server.mjs: the existing Full E2E suite keeps
// its own port, state, and HTTP-only assumptions.
import { spawn, execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { once } from "node:events";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { closeHttpsProxy, startHttpsProxy } from "./demo-proxy.mjs";

const BACKEND_PORT = 18788;
const PROXY_PORT = 18789;
const BACKEND_HOST = "127.0.0.1";
const PROXY_HOST = "127.0.0.1";
const BASE_URL = `https://${PROXY_HOST}:${PROXY_PORT}`;
const STARTUP_TIMEOUT_MS = 30_000;
const SHUTDOWN_TIMEOUT_MS = 5_000;

const here = fileURLToPath(new URL(".", import.meta.url));
const repoRoot = resolve(here, "..", "..");
const exeName =
  process.platform === "win32" ? "bhtune-server.exe" : "bhtune-server";
const bin = resolve(repoRoot, "target", "debug", exeName);
const stateDir = resolve(here, "..", "test-results", "server-state", "demo");
const configPath = join(stateDir, "bhtune.toml");
const databasePath = join(stateDir, "bhtune.db");
const logDir = join(stateDir, "logs");
const certificatePath = join(stateDir, "localhost.crt");
const keyPath = join(stateDir, "localhost.key");

let backend;
let proxy;
let shuttingDown = false;

function tomlString(value) {
  return JSON.stringify(value);
}

function isolatedEnvironment() {
  const scrubbedNames = new Set([
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "HOME",
    "LOCALAPPDATA",
    "USERPROFILE",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
  ]);
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(
      ([name]) => !name.startsWith("BHTUNE_") && !scrubbedNames.has(name),
    ),
  );

  return {
    ...environment,
    APPDATA: stateDir,
    HOME: stateDir,
    LOCALAPPDATA: stateDir,
    USERPROFILE: stateDir,
    XDG_CONFIG_HOME: stateDir,
    XDG_DATA_HOME: stateDir,
    XDG_STATE_HOME: stateDir,
    RUST_LOG: "info",
    BHTUNE_BIND: `${BACKEND_HOST}:${BACKEND_PORT}`,
    BHTUNE_DB: databasePath,
    BHTUNE_ORIGIN: BASE_URL,
    BHTUNE_SERVER_MODE: "demo",
  };
}

function writeDemoConfig() {
  writeFileSync(
    configPath,
    [
      'server_mode = "demo"',
      `bind = ${tomlString(`${BACKEND_HOST}:${BACKEND_PORT}`)}`,
      `origin = ${tomlString(BASE_URL)}`,
      'trusted_proxy = "127.0.0.1"',
      "[log]",
      `dir = ${tomlString(logDir)}`,
      'level = "info"',
      'rotation = "never"',
      "",
    ].join("\n"),
  );
}

function createCertificate() {
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      "1",
      "-keyout",
      keyPath,
      "-out",
      certificatePath,
      "-subj",
      "/CN=127.0.0.1",
      "-addext",
      "subjectAltName=IP:127.0.0.1",
    ],
    { stdio: ["ignore", "ignore", "pipe"] },
  );
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function waitForBackend() {
  const deadline = Date.now() + STARTUP_TIMEOUT_MS;
  let lastError = "no response";
  while (Date.now() < deadline) {
    if (backend.exitCode !== null || backend.signalCode !== null) {
      throw new Error(
        `bhtune-server exited before readiness (code=${backend.exitCode}, signal=${backend.signalCode})`,
      );
    }
    try {
      const response = await fetch(
        `http://${BACKEND_HOST}:${BACKEND_PORT}/api/health`,
      );
      if (response.ok) return;
      lastError = `HTTP ${response.status}`;
    } catch (error) {
      lastError = error instanceof Error ? error.message : String(error);
    }
    await delay(100);
  }
  throw new Error(
    `bhtune-server did not become ready within ${STARTUP_TIMEOUT_MS}ms: ${lastError}`,
  );
}

async function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return true;

  let timer;
  const exit = once(child, "exit");
  const timeout = new Promise((resolve) => {
    timer = setTimeout(() => resolve(false), timeoutMs);
  });
  const result = await Promise.race([exit.then(() => true), timeout]);
  clearTimeout(timer);
  return result;
}

async function shutdown(exitCode) {
  if (shuttingDown) return;
  shuttingDown = true;

  try {
    if (proxy) await closeHttpsProxy(proxy);
  } catch (error) {
    console.error(
      `e2e: failed to close Demo HTTPS proxy: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
    exitCode = 1;
  }

  if (backend && backend.exitCode === null && backend.signalCode === null) {
    backend.kill("SIGTERM");
    if (!(await waitForExit(backend, SHUTDOWN_TIMEOUT_MS))) {
      backend.kill("SIGKILL");
      await waitForExit(backend, 1_000);
      exitCode = 1;
    }
  }

  rmSync(stateDir, { recursive: true, force: true });
  process.exitCode = exitCode;
}

async function main() {
  if (!existsSync(bin)) {
    throw new Error(
      `e2e: ${bin} does not exist -- build it first: cargo build -p bhtune-server`,
    );
  }
  if (!existsSync(join(repoRoot, "frontend", "dist", "index.html"))) {
    throw new Error(
      "e2e: frontend/dist/index.html does not exist -- build it first: pnpm --filter bhtune-frontend run build",
    );
  }

  rmSync(stateDir, { recursive: true, force: true });
  mkdirSync(logDir, { recursive: true });
  writeDemoConfig();
  createCertificate();

  console.log(
    `e2e: starting isolated Demo backend on http://${BACKEND_HOST}:${BACKEND_PORT} and HTTPS proxy on ${BASE_URL}`,
  );

  backend = spawn(bin, ["--config", configPath], {
    env: isolatedEnvironment(),
    stdio: "inherit",
  });
  backend.once("error", (error) => {
    console.error(`e2e: failed to spawn ${bin}: ${error.message}`);
    void shutdown(1);
  });
  backend.once("exit", (code, signal) => {
    if (!shuttingDown) {
      console.error(
        `e2e: Demo backend exited unexpectedly (code=${code}, signal=${signal})`,
      );
      void shutdown(code ?? 1);
    }
  });

  await waitForBackend();
  proxy = await startHttpsProxy({
    listenHost: PROXY_HOST,
    listenPort: PROXY_PORT,
    backendHost: BACKEND_HOST,
    backendPort: BACKEND_PORT,
    keyPath,
    certificatePath,
  });
  console.log(`e2e: Demo HTTPS proxy ready at ${BASE_URL}`);

  await new Promise(() => {});
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    void shutdown(0);
  });
}

try {
  await main();
} catch (error) {
  console.error(
    `e2e: failed to start Demo environment: ${
      error instanceof Error ? error.message : String(error)
    }`,
  );
  await shutdown(1);
}
