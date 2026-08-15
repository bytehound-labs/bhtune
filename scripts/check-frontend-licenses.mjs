#!/usr/bin/env node
// Enforces the same "no proprietary/licensed dependencies" guarantee that
// `cargo deny check` machine-enforces for Rust crates (see deny.toml, whose
// allowlist this mirrors) -- but for the frontend's npm dependency tree.
// Uses pnpm's built-in `licenses list` (no extra npm dependency needed) and
// fails if any installed package's license is not on the allowlist below.
//
// If this fails on a new dependency, the fix is almost always to find an
// alternative package, not to widen this list -- widening it should be a
// deliberate, reviewed decision, never a reflex to unblock CI.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const ALLOWED_LICENSES = new Set([
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unlicense",
  "CC0-1.0",
  "Zlib",
  "0BSD",
  "Unicode-3.0",
  "AGPL-3.0-or-later",
  "MPL-2.0",
  // PSF license (Python Software Foundation) -- OSI-approved and permissive.
  // Pulled in transitively by `argparse` (a JS port of Python's argparse
  // that kept its upstream license tag).
  "Python-2.0",
]);

const repoRoot = path.resolve(fileURLToPath(import.meta.url), "..", "..");

const raw = execFileSync("pnpm", ["licenses", "list", "--json"], {
  cwd: repoRoot,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});

const licenses = JSON.parse(raw);

// SPDX "OR" expressions (e.g. "(MIT OR CC0-1.0)") are satisfiable if any one
// arm is on the allowlist.
function isAllowed(licenseExpr) {
  const arms = licenseExpr
    .replace(/[()]/g, "")
    .split(/\s+OR\s+/i)
    .map((arm) => arm.trim());
  return arms.some((arm) => ALLOWED_LICENSES.has(arm));
}

const violations = [];
let total = 0;
for (const [license, packages] of Object.entries(licenses)) {
  total += packages.length;
  if (isAllowed(license)) continue;
  for (const pkg of packages) {
    violations.push(`${pkg.name}@${pkg.versions.join(", ")}: ${license}`);
  }
}

if (violations.length > 0) {
  console.error(
    "The following npm dependencies use a license not on the allowlist:",
  );
  for (const violation of violations) {
    console.error(`  - ${violation}`);
  }
  console.error(
    "\nSee scripts/check-frontend-licenses.mjs to review or (deliberately) widen the allowlist.",
  );
  process.exit(1);
}

console.log(`All ${total} npm packages use an allowed license.`);
