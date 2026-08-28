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
  // MIT "No Attribution" -- OSI-approved, strictly more permissive than MIT
  // (drops the attribution-notice requirement). Pulled in by several
  // @csstools/postcss-* packages via the Docusaurus/website toolchain.
  "MIT-0",
  // Creative Commons Attribution 4.0 -- a content (not code) license, used by
  // `caniuse-lite` for its browser-support data tables. Permissive
  // (attribution-only, no share-alike), and an unavoidable transitive
  // dependency of browserslist/postcss-preset-env across the JS ecosystem.
  "CC-BY-4.0",
  // Mozilla Public License 1.1 -- OSI-approved, same weak/file-level
  // copyleft family as MPL-2.0 above (its successor, already allowed).
  // Pulled in by `lunr-languages` (docusaurus-search-local's search index).
  "MPL-1.1",
  // Blue Oak Model License 1.0 -- not OSI-approved but reviewed and endorsed
  // by the Blue Oak Council; functionally at least as permissive as MIT
  // (explicit patent grant, no-liability, single attribution-notice
  // requirement). Used by `sax` (an npm/Isaac-Z-Schlueter-authored package).
  "BlueOak-1.0.0",
]);

// Packages whose license metadata `pnpm licenses list` cannot resolve
// ("Unknown"), each manually verified by reading the package's actual
// license file/text -- keyed by exact name+version so a genuinely
// unknown/undeclared license on any *other* package (or a future version of
// these) still fails loudly rather than being silently let through.
const VERIFIED_UNKNOWN_LICENSES = new Map([
  // No `license` field in package.json and no SPDX-recognized LICENSE
  // filename, but its `License` file is verbatim, unmodified MIT license
  // text. Pulled in transitively (docusaurus -> webpack -> enhanced-resolve
  // or similar). Verified by direct inspection on 2026-08-16.
  [
    "require-like@0.1.2",
    "MIT (undeclared -- verified by reading LICENSE text)",
  ],
]);

const repoRoot = path.resolve(fileURLToPath(import.meta.url), "..", "..");

const raw = execFileSync("pnpm", ["licenses", "list", "--json"], {
  cwd: repoRoot,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});

const licenses = JSON.parse(raw);

// SPDX license expressions combine arms with OR (any one arm suffices, e.g.
// "(MIT OR CC0-1.0)") or AND (every arm's terms apply simultaneously, e.g.
// "Apache-2.0 AND MIT" for a dual-licensed package) -- an AND expression is
// safe iff *all* arms are already individually allowed.
function splitOnOperator(tokens, operator) {
  const arms = [];
  let current = [];
  for (const token of tokens) {
    if (token.toUpperCase() === operator) {
      arms.push(current.join(" "));
      current = [];
    } else {
      current.push(token);
    }
  }
  arms.push(current.join(" "));
  return arms;
}

function isAllowed(licenseExpr) {
  const stripped = licenseExpr.replaceAll("(", "").replaceAll(")", "").trim();
  const tokens = stripped.length === 0 ? [] : stripped.split(/\s+/);
  if (tokens.some((token) => token.toUpperCase() === "OR")) {
    const arms = splitOnOperator(tokens, "OR");
    return arms.some((arm) => ALLOWED_LICENSES.has(arm));
  }
  if (tokens.some((token) => token.toUpperCase() === "AND")) {
    const arms = splitOnOperator(tokens, "AND");
    return arms.every((arm) => ALLOWED_LICENSES.has(arm));
  }
  return ALLOWED_LICENSES.has(stripped);
}

const violations = [];
let total = 0;
for (const [license, packages] of Object.entries(licenses)) {
  total += packages.length;
  if (isAllowed(license)) continue;
  for (const pkg of packages) {
    // "Unknown" (or any other unresolved license) may still be fine if
    // every installed version of this exact package was manually verified
    // above -- check each version individually rather than trusting the
    // license string, since a *different* future version could genuinely
    // ship an unknown/proprietary license.
    const unverifiedVersions = pkg.versions.filter(
      (version) => !VERIFIED_UNKNOWN_LICENSES.has(`${pkg.name}@${version}`),
    );
    if (unverifiedVersions.length === 0) continue;
    violations.push(`${pkg.name}@${unverifiedVersions.join(", ")}: ${license}`);
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
