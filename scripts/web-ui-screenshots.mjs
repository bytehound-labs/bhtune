#!/usr/bin/env node

import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const manifestPath = resolve(root, "docs/reference/web-ui-screenshots.json");
const screenshotDir = resolve(root, "website/static/generated/web-ui");
const galleryDir = resolve(root, "frontend/test-results/docs-screenshots");
const galleryScreenshotsDir = join(galleryDir, "screenshots");
const pagesBaseUrl = "https://bytehound-labs.github.io/bhtune/generated/web-ui";
const publishedScreenshotUrlPattern =
  /https:\/\/bytehound-labs\.github\.io\/bhtune\/generated\/web-ui\/([a-z0-9-]+\.png)(?:\?v=[0-9a-f]+)?/g;
const binaryExtensions = /\.(?:png|jpe?g|gif|webp|mp4|webm|mov)$/i;

const command = process.argv[2] ?? "capture";
const shouldUpdateLock = process.argv.includes("--update-lock");

function readManifest() {
  return JSON.parse(readFileSync(manifestPath, "utf8"));
}

function writeManifest(manifest) {
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

function fail(message) {
  console.error(`web-ui screenshots: ${message}`);
  process.exitCode = 1;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function pngDimensions(path) {
  const bytes = readFileSync(path);
  const signature = Buffer.from([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
  ]);
  if (bytes.length < 24 || !bytes.subarray(0, 8).equals(signature)) {
    throw new Error(`${path} is not a PNG file`);
  }
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
  };
}

function publicUrl(output, hash) {
  return `${pagesBaseUrl}/${output}?v=${hash.slice(0, 12)}`;
}

function markdownFiles() {
  const files = [
    "README.md",
    "CONTRIBUTING.md",
    "frontend/README.md",
    "website/README.md",
  ];

  function visit(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        visit(path);
      } else if (entry.isFile() && entry.name.endsWith(".md")) {
        files.push(relative(root, path));
      }
    }
  }

  visit(resolve(root, "docs"));
  return [...new Set(files)];
}

function frontendSourceFiles() {
  const files = [];

  function visit(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        visit(path);
      } else if (entry.isFile() && /\.(?:ts|tsx)$/.test(entry.name)) {
        files.push(path);
      }
    }
  }

  visit(resolve(root, "frontend/src"));
  return files;
}

function sourceDocumentationSectionIds() {
  const ids = new Set();
  const markerPattern =
    /(?:documentationId|data-doc-section)\s*=\s*(?:\{\s*)?["']([a-z0-9-]+(?:\.[a-z0-9-]+)+)["'](?:\s*\})?/g;

  for (const path of frontendSourceFiles()) {
    const content = readFileSync(path, "utf8");
    for (const match of content.matchAll(markerPattern)) {
      ids.add(match[1]);
    }
  }

  return ids;
}

function rewriteDocumentationLinks(manifest) {
  const scenariosByOutput = new Map(
    manifest.scenarios.map((scenario) => [scenario.output, scenario]),
  );

  for (const path of markdownFiles()) {
    const absolutePath = resolve(root, path);
    if (!existsSync(absolutePath)) continue;
    const content = readFileSync(absolutePath, "utf8");
    const updated = content.replace(
      publishedScreenshotUrlPattern,
      (match, output) => {
        const scenario = scenariosByOutput.get(output);
        return scenario ? publicUrl(scenario.output, scenario.sha256) : match;
      },
    );
    writeFileSync(absolutePath, updated);
  }
}

function validateManifestShape(manifest, errors) {
  if (manifest.schemaVersion !== 1) {
    errors.push(
      `unsupported manifest schemaVersion: ${manifest.schemaVersion}`,
    );
  }
  if (!Array.isArray(manifest.scenarios) || manifest.scenarios.length === 0) {
    errors.push("manifest has no scenarios");
    return null;
  }
  return manifest.scenarios;
}

function validateSectionCoverage(scenarios, errors) {
  const manifestSectionIds = new Set(
    scenarios.flatMap((scenario) => scenario.coveredSections ?? []),
  );
  const sourceSectionIds = sourceDocumentationSectionIds();
  for (const id of sourceSectionIds) {
    if (!manifestSectionIds.has(id)) {
      errors.push(
        `frontend documentation section has no screenshot coverage: ${id}`,
      );
    }
  }
  for (const id of manifestSectionIds) {
    if (!sourceSectionIds.has(id)) {
      errors.push(
        `screenshot manifest references missing frontend documentation section: ${id}`,
      );
    }
  }
}

function validateScenarioMetadata(scenario, ids, outputs, errors) {
  if (ids.has(scenario.id)) {
    errors.push(`duplicate scenario id: ${scenario.id}`);
  }
  ids.add(scenario.id);

  if (!/^[a-z0-9-]+\.png$/.test(scenario.output)) {
    errors.push(
      `${scenario.id} has an unsafe output filename: ${scenario.output}`,
    );
  }
  if (outputs.has(scenario.output)) {
    errors.push(`duplicate screenshot output: ${scenario.output}`);
  }
  outputs.add(scenario.output);

  if (
    !Array.isArray(scenario.coveredSections) ||
    scenario.coveredSections.length === 0
  ) {
    errors.push(`${scenario.id} has no covered UI sections`);
  }
  if (
    !Array.isArray(scenario.documentation) ||
    scenario.documentation.length === 0
  ) {
    errors.push(`${scenario.id} has no documentation references`);
  }
}

function validateScenarioImage(scenario, requireHashes, errors) {
  const path = join(screenshotDir, scenario.output);
  if (!existsSync(path)) {
    errors.push(
      `${scenario.id} is missing generated output: ${scenario.output}`,
    );
    return;
  }

  try {
    const dimensions = pngDimensions(path);
    const actualHash = sha256(path);
    if (requireHashes && scenario.sha256 !== actualHash) {
      errors.push(
        `${scenario.id} hash mismatch: manifest ${scenario.sha256 ?? "<missing>"} != ${actualHash}`,
      );
    }
    if (requireHashes && scenario.width !== dimensions.width) {
      errors.push(
        `${scenario.id} width mismatch: manifest ${scenario.width ?? "<missing>"} != ${dimensions.width}`,
      );
    }
    if (requireHashes && scenario.height !== dimensions.height) {
      errors.push(
        `${scenario.id} height mismatch: manifest ${scenario.height ?? "<missing>"} != ${dimensions.height}`,
      );
    }
    if (
      requireHashes &&
      scenario.publicUrl !== publicUrl(scenario.output, actualHash)
    ) {
      errors.push(`${scenario.id} has a stale publicUrl`);
    }
  } catch (error) {
    errors.push(error instanceof Error ? error.message : String(error));
  }
}

function validateScenarioImages(scenarios, requireHashes, errors) {
  const ids = new Set();
  const outputs = new Set();
  for (const scenario of scenarios) {
    validateScenarioMetadata(scenario, ids, outputs, errors);
    validateScenarioImage(scenario, requireHashes, errors);
  }
  return outputs;
}

function validateGeneratedOutputs(outputs, errors) {
  if (!existsSync(screenshotDir)) return;
  for (const entry of readdirSync(screenshotDir)) {
    if (entry.endsWith(".png") && !outputs.has(entry)) {
      errors.push(`unexpected generated screenshot: ${entry}`);
    }
  }
}

function validateDocumentationMarkers(scenarios, requireHashes, errors) {
  const referencedIds = new Set();
  const scenarioById = new Map(
    scenarios.map((scenario) => [scenario.id, scenario]),
  );
  const markerPattern =
    /(?:<!--\s*web-ui-screenshot:\s*([a-z0-9-]+)\s*-->|{\/\*\s*web-ui-screenshot:\s*([a-z0-9-]+)\s*\*\/})/g;

  for (const path of markdownFiles()) {
    const absolutePath = resolve(root, path);
    if (!existsSync(absolutePath)) continue;
    const content = readFileSync(absolutePath, "utf8");
    for (const match of content.matchAll(markerPattern)) {
      const id = match[1] ?? match[2];
      const scenario = scenarioById.get(id);
      if (!scenario) {
        errors.push(`${path} references unknown screenshot scenario: ${id}`);
        continue;
      }
      referencedIds.add(id);
      if (!(scenario.documentation ?? []).includes(path)) {
        errors.push(
          `${path} is not listed in ${id}'s documentation references`,
        );
      }
      const expectedUrl =
        requireHashes && scenario.sha256
          ? publicUrl(scenario.output, scenario.sha256)
          : `${pagesBaseUrl}/${scenario.output}`;
      if (!content.includes(expectedUrl)) {
        errors.push(`${path} has no image URL for screenshot scenario ${id}`);
      }
    }
  }
  return referencedIds;
}

function validateDocumentationReferences(scenarios, referencedIds, errors) {
  for (const scenario of scenarios) {
    if (!referencedIds.has(scenario.id)) {
      errors.push(
        `screenshot scenario has no documentation marker: ${scenario.id}`,
      );
    }
    for (const path of scenario.documentation ?? []) {
      const absolutePath = resolve(root, path);
      if (!existsSync(absolutePath)) {
        errors.push(
          `${scenario.id} references missing documentation file: ${path}`,
        );
      }
    }
  }
}

function validateManifest(manifest, { requireHashes = true } = {}) {
  const errors = [];
  const scenarios = validateManifestShape(manifest, errors);
  if (!scenarios) return errors;

  validateSectionCoverage(scenarios, errors);
  const outputs = validateScenarioImages(scenarios, requireHashes, errors);
  validateGeneratedOutputs(outputs, errors);
  const referencedIds = validateDocumentationMarkers(
    scenarios,
    requireHashes,
    errors,
  );
  validateDocumentationReferences(scenarios, referencedIds, errors);
  return errors;
}

function trustedGitExecutable() {
  const candidates =
    process.platform === "win32"
      ? [
          resolve(
            process.env.ProgramW6432 ?? "C:\\Program Files",
            "Git",
            "cmd",
            "git.exe",
          ),
          resolve(
            process.env.ProgramFiles ?? "C:\\Program Files",
            "Git",
            "cmd",
            "git.exe",
          ),
          resolve(
            process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)",
            "Git",
            "cmd",
            "git.exe",
          ),
        ]
      : ["/usr/bin/git", "/usr/local/bin/git", "/bin/git"];
  const executable = candidates.find((candidate) => existsSync(candidate));
  if (!executable) {
    throw new Error("could not find Git in a trusted system location");
  }
  return executable;
}

function rejectTrackedBinaries() {
  const tracked = execFileSync(trustedGitExecutable(), ["ls-files", "-z"], {
    cwd: root,
    encoding: "buffer",
  })
    .toString("utf8")
    .split("\0")
    .filter(Boolean);
  return tracked
    .filter(
      (path) =>
        binaryExtensions.test(path) &&
        (path.startsWith("docs/") ||
          path.startsWith("website/static/generated/") ||
          path.startsWith("frontend/e2e/docs-screenshots/")),
    )
    .map((path) => `tracked screenshot/media binary: ${path}`);
}

function generateGallery(manifest) {
  rmSync(galleryDir, { recursive: true, force: true });
  mkdirSync(galleryScreenshotsDir, { recursive: true });

  const cards = manifest.scenarios
    .map((scenario) => {
      const source = join(screenshotDir, scenario.output);
      copyFileSync(source, join(galleryScreenshotsDir, scenario.output));
      const candidateUrl = `screenshots/${scenario.output}`;
      const liveUrl =
        scenario.publicUrl ?? `${pagesBaseUrl}/${scenario.output}`;
      return `
        <article class="card">
          <h2>${escapeHtml(scenario.title)}</h2>
          <p><strong>${escapeHtml(scenario.mode)}</strong> · <code>${escapeHtml(
            scenario.route,
          )}</code> · ${escapeHtml(scenario.state)}</p>
          <div class="comparison">
            <figure>
              <figcaption>Candidate</figcaption>
              <a href="${candidateUrl}"><img src="${candidateUrl}" alt="${escapeHtml(
                scenario.title,
              )} candidate screenshot"></a>
            </figure>
            <figure>
              <figcaption>Published reference</figcaption>
              <a href="${liveUrl}"><img src="${liveUrl}" alt="${escapeHtml(
                scenario.title,
              )} published reference screenshot"></a>
            </figure>
          </div>
          <p class="meta"><code>${escapeHtml(scenario.id)}</code> · ${
            scenario.sha256 ?? "hash not locked"
          }</p>
        </article>`;
    })
    .join("\n");

  writeFileSync(
    join(galleryDir, "index.html"),
    `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>BHTune Web UI screenshot review</title>
  <style>
    :root { color-scheme: dark; font-family: system-ui, sans-serif; }
    body { margin: 0; padding: 2rem; background: #111827; color: #e5e7eb; }
    h1 { margin-top: 0; }
    .card { margin: 2rem 0; padding: 1rem; border: 1px solid #374151; border-radius: .75rem; background: #1f2937; }
    .comparison { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
    figure { margin: 0; }
    figcaption { margin-bottom: .5rem; font-weight: 700; }
    img { display: block; width: 100%; height: auto; border: 1px solid #4b5563; }
    code { color: #c4b5fd; }
    .meta { color: #9ca3af; overflow-wrap: anywhere; }
    @media (max-width: 900px) { .comparison { grid-template-columns: 1fr; } }
  </style>
</head>
<body>
  <h1>BHTune Web UI screenshot review</h1>
  <p>Candidate screenshots are generated from the canonical dark desktop profile. The second
  column loads the deployed Pages image for visual comparison when it exists.</p>
  ${cards}
</body>
</html>
`,
  );
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function capture(mode) {
  const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
  const result = spawnSync(
    pnpm,
    [
      "--filter",
      "bhtune-frontend",
      "exec",
      "playwright",
      "test",
      "--config",
      "playwright.docs.config.ts",
    ],
    {
      cwd: root,
      env: {
        ...process.env,
        DOCS_MODE: mode,
        DOCS_SCREENSHOT_DIR: screenshotDir,
      },
      stdio: "inherit",
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Playwright documentation capture failed for ${mode}`);
  }
}

function updateManifestLock(manifest) {
  for (const scenario of manifest.scenarios) {
    const path = join(screenshotDir, scenario.output);
    const dimensions = pngDimensions(path);
    const hash = sha256(path);
    scenario.sha256 = hash;
    scenario.width = dimensions.width;
    scenario.height = dimensions.height;
    scenario.publicUrl = publicUrl(scenario.output, hash);
  }
  writeManifest(manifest);
  rewriteDocumentationLinks(manifest);
}

function validate(manifest, requireHashes = true) {
  const errors = [
    ...rejectTrackedBinaries(),
    ...validateManifest(manifest, { requireHashes }),
  ];
  if (errors.length > 0) {
    for (const error of errors) console.error(`- ${error}`);
    throw new Error(
      requireHashes
        ? "screenshot metadata or documentation references are invalid"
        : "screenshot metadata is invalid",
    );
  }
}

try {
  if (command === "capture") {
    rmSync(screenshotDir, { recursive: true, force: true });
    mkdirSync(screenshotDir, { recursive: true });
    capture("full");
    capture("demo");
    const manifest = readManifest();
    if (shouldUpdateLock) {
      updateManifestLock(manifest);
    }
    validate(manifest);
    generateGallery(manifest);
    console.log(
      `Generated ${manifest.scenarios.length} Web UI screenshots and ${join(
        relative(root, galleryDir),
        "index.html",
      )}.`,
    );
  } else if (command === "validate") {
    validate(readManifest());
  } else if (command === "gallery") {
    const manifest = readManifest();
    validate(manifest);
    generateGallery(manifest);
  } else {
    throw new Error(`unknown command: ${command}`);
  }
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}
