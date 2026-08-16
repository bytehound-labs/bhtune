import type { Config } from "@docusaurus/types";
import type * as Preset from "@docusaurus/preset-classic";
import { themes as prismThemes } from "prism-react-renderer";

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

const config: Config = {
  title: "BHTune",
  tagline: "Open-source PID auto-tuning for industrial DCS/PLC loops",
  favicon: "img/favicon.svg",

  future: {
    v4: true, // Improve compatibility with the upcoming Docusaurus v4
  },

  // GitHub Pages deployment config (see docs-site-deploy for the publish workflow).
  url: "https://bytehound-labs.github.io",
  baseUrl: "/bhtune/",
  organizationName: "bytehound-labs",
  projectName: "bhtune",

  // A doc referencing a renamed/deleted page fails the build rather than
  // shipping a dead link -- this is the cheapest drift gate this site has.
  onBrokenLinks: "throw",
  onBrokenAnchors: "throw",

  i18n: {
    defaultLocale: "en",
    locales: ["en"],
  },

  presets: [
    [
      "classic",
      {
        docs: {
          // The content root is the real docs/ at the repo root, not a
          // website-local copy -- this is the single source of truth also
          // read directly on GitHub, so a code change and its doc change
          // land in the same commit. See AGENTS.md's "Documentation system".
          path: "../docs",
          routeBasePath: "/", // No separate marketing page; docs/intro.md is the home page.
          sidebarPath: "./sidebars.ts",
          exclude: ["internal/**"], // docs/internal/ is not published (see its own note).
          // A plain string editUrl naively concatenates with `path` above and produces
          // .../edit/main/docs/../docs/intro.md; the function form gets the real
          // repo-relative path directly instead.
          editUrl: ({ docPath }) =>
            `https://github.com/bytehound-labs/bhtune/edit/main/docs/${docPath}`,
        },
        blog: false,
        theme: {
          customCss: "./src/css/custom.css",
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: "BHTune",
      logo: {
        alt: "BHTune logo",
        src: "img/favicon.svg",
      },
      items: [
        {
          href: "https://github.com/bytehound-labs/bhtune",
          label: "GitHub",
          position: "right",
        },
      ],
    },
    footer: {
      style: "dark",
      links: [
        {
          title: "Docs",
          items: [
            { label: "Introduction", to: "/" },
            { label: "Getting started", to: "/getting-started/installation" },
            { label: "DCS/PLC templates", to: "/dcs-templates" },
            { label: "Rust API reference", to: "/reference/api" },
          ],
        },
        {
          title: "Project",
          items: [
            {
              label: "GitHub",
              href: "https://github.com/bytehound-labs/bhtune",
            },
            {
              label: "Issues",
              href: "https://github.com/bytehound-labs/bhtune/issues",
            },
            {
              label: "License (AGPL-3.0-or-later)",
              href: "https://github.com/bytehound-labs/bhtune/blob/main/LICENSE",
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} ByteHound. Released under the AGPL-3.0-or-later license.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
      additionalLanguages: ["toml", "bash", "rust"],
    },
  } satisfies Preset.ThemeConfig,

  themes: ["@easyops-cn/docusaurus-search-local"],
};

export default config;
