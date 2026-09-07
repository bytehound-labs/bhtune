#!/usr/bin/env node
// Spawns a real, already-compiled `bhtune-server` binary (debug profile -- see
// `crates/bhtune-server/Cargo.toml`'s `rust-embed` dependency comment: without `--release`
// it reads `frontend/dist/` live off disk on every request, so this script's caller only
// has to build the frontend once and `cargo build -p bhtune-server` once, not re-embed
// anything between runs) against a fresh SQLite database and isolated XDG state directories
// under Playwright's ignored `test-results/` tree, bound to a fixed port so
// `playwright.config.ts`'s `webServer.url` can poll it deterministically.
//
// This is `playwright.config.ts`'s `webServer.command` -- Playwright starts it before the
// test run, waits for its `BASE_URL`'s `/api/health` to answer, and tears it down after.
// Mirrors `crates/bhtune-server/tests/graceful_shutdown.rs`'s subprocess-spawning pattern
// (temp DB, temp XDG dirs, `BHTUNE_BIND` env var) from the Rust side, translated to Node
// since this runs from the frontend workspace.
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Distinct from the app's own documented default (127.0.0.1:8787), so this suite never
// collides with a developer's already-running `bhtune-server`/`pnpm dev` instance.
const PORT = 18787;

const here = fileURLToPath(new URL(".", import.meta.url));
const repoRoot = resolve(here, "..", "..");
const exeName =
  process.platform === "win32" ? "bhtune-server.exe" : "bhtune-server";
const bin = resolve(repoRoot, "target", "debug", exeName);

if (!existsSync(bin)) {
  console.error(
    `e2e: ${bin} does not exist -- build it first: cargo build -p bhtune-server`,
  );
  process.exit(1);
}

const stateDir = resolve(here, "..", "test-results", "server-state", "full");
rmSync(stateDir, { recursive: true, force: true });
const configDir = join(stateDir, "bhtune");
mkdirSync(configDir, { recursive: true });
writeFileSync(
  join(configDir, "bhtune.toml"),
  "[tuning]\npoll_interval_ms = 5\ntimeout_secs = 30\n",
);
console.log(
  `e2e: starting ${bin} on 127.0.0.1:${PORT} (state dir: ${stateDir})`,
);

const child = spawn(bin, [], {
  env: {
    ...process.env,
    BHTUNE_DB: join(stateDir, "bhtune.db"),
    BHTUNE_BIND: `127.0.0.1:${PORT}`,
    // Redirects every other XDG-style default (log dir, user template catalog) into the
    // same throwaway directory -- without this, `bhtune-server` would resolve real
    // platform defaults (e.g. `~/.local/share/bhtune/`) using this process's inherited
    // `HOME`, exactly as `graceful_shutdown.rs`'s own comment warns against.
    XDG_DATA_HOME: stateDir,
    XDG_CONFIG_HOME: stateDir,
  },
  stdio: "inherit",
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.exit(0);
  }
  process.exit(code ?? 0);
});

child.on("error", (err) => {
  console.error(`e2e: failed to spawn ${bin}: ${err.message}`);
  process.exit(1);
});

// Playwright stops a `webServer` process by sending it a signal once the test run ends;
// forward that straight to the child so it goes through its own graceful-shutdown path
// (`main.rs`'s `shutdown_signal()`) instead of being orphaned.
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}
