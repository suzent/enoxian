import fs from "node:fs";
import path from "node:path";
import { createContent, root, repo, site, locales } from "./content.mjs";

const content = createContent();

// 1. Sync shared visual modules from ../frontend so there is zero drift
const frontendSrc = path.join(repo, "frontend/src");
const websiteComponents = path.join(root, "src/components");
fs.mkdirSync(websiteComponents, { recursive: true });

const ditherSrc = fs
  .readFileSync(path.join(frontendSrc, "lib/ditherShader.ts"), "utf8");
fs.writeFileSync(
  path.join(websiteComponents, "shared-dither.ts"),
  "// Auto-synced from ../frontend/src/lib/ditherShader.ts by scripts/prepare.mjs\n" +
    ditherSrc,
);

const angelSrc = fs
  .readFileSync(path.join(frontendSrc, "components/AngelScene.ts"), "utf8")
  .replace("from '../lib/ditherShader'", "from './shared-dither'")
  .replace('from "../lib/ditherShader"', 'from "./shared-dither"');
fs.writeFileSync(
  path.join(websiteComponents, "shared-angel.ts"),
  "// Auto-synced from ../frontend/src/components/AngelScene.ts by scripts/prepare.mjs\n" +
    angelSrc,
);

const circleIdentitySrc = fs.readFileSync(
  path.join(frontendSrc, "lib/circleIdentity.ts"),
  "utf8",
);
fs.writeFileSync(
  path.join(websiteComponents, "shared-circle-identity.ts"),
  "// Auto-synced from ../frontend/src/lib/circleIdentity.ts by scripts/prepare.mjs\n" +
    circleIdentitySrc,
);

// 2. Emit generated content index
fs.mkdirSync(path.join(root, ".generated"), { recursive: true });
fs.writeFileSync(
  path.join(root, ".generated/content.json"),
  JSON.stringify(content),
);

// 3. Populate public/ assets
fs.mkdirSync(path.join(root, "public"), { recursive: true });
if (fs.existsSync(path.join(root, "static"))) {
  fs.cpSync(path.join(root, "static"), path.join(root, "public"), {
    recursive: true,
  });
}
const logoPath = path.join(repo, "frontend/public/logo.svg");
if (fs.existsSync(logoPath)) {
  fs.mkdirSync(path.join(root, "public/img"), { recursive: true });
  fs.copyFileSync(logoPath, path.join(root, "public/img/logo.svg"));
}
const docsAssets = path.join(repo, "docs/assets");
if (fs.existsSync(docsAssets)) {
  fs.cpSync(
    docsAssets,
    path.join(root, "public/content-assets/docs/assets"),
    {
      recursive: true,
      filter: (source) =>
        fs.statSync(source).isDirectory() ||
        /\.(png|jpg|jpeg|svg|webp|gif|avif)$/i.test(source),
    },
  );
}

const xml = (value) =>
  String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");

const write = (relUrl, text) => {
  const file = path.join(root, "public", relUrl);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
};

write(".nojekyll", "");
write("CNAME", "enoxian.com\n");
write(
  "robots.txt",
  `User-agent: *\nAllow: /\nSitemap: ${site}/sitemap.xml\n`,
);
write(
  "sitemap.xml",
  '<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">' +
    content.pages
      .filter((p) => p.kind !== "notfound" && p.kind !== "redirect")
      .map(
        (p) =>
          `<url><loc>${site}${xml(p.url === "/" ? "/" : p.url + "/")}</loc></url>`,
      )
      .join("") +
    "</urlset>",
);

const overviewSummary = {
  en: "Enoxian brings shared notes, conversations, tasks, and AI agents into a peer-to-peer Circle. Every device keeps its own copy of the shared folder, works offline, and syncs peer to peer when it reconnects.",
  "zh-Hans":
    "Enoxian 将共享笔记、对话、任务与 AI 智能体汇聚于点对点共享空间（Circle）。每台设备保留各自的文件夹副本，支持离线工作并在重连后点对点同步。",
};

for (const locale of locales) {
  const prefix = locale === "en" ? "" : locale + "/";
  const pages = content.pages.filter((p) => p.locale === locale);
  write(
    prefix + "search-index.json",
    JSON.stringify(
      pages
        .filter((p) => p.content)
        .map((p) => ({ url: p.url, title: p.title, text: p.searchText })),
    ),
  );
  const docs = pages.filter((p) => p.kind === "doc");
  write(
    prefix + "llms.txt",
    "# Enoxian\n\n" +
      overviewSummary[locale] +
      "\n\n" +
      `Full documentation: ${site}/${prefix}llms-full.txt\n\n` +
      docs.map((p) => `- [${p.title}](${site}${p.url}/)`).join("\n") +
      "\n",
  );
  write(
    prefix + "llms-full.txt",
    "# Enoxian — Peer-to-Peer Circles for Humans and Agents\n\n" +
      overviewSummary[locale] +
      "\n\n" +
      docs
        .map(
          (p) =>
            `---\n\n# ${p.title}\n\nSource: ${site}${p.url}/\n\n${p.content.replace(/\]\(\//g, "](" + site + "/")}`,
        )
        .join("\n\n"),
  );
}

console.log(
  `Prepared ${content.pages.length} routes (${content.pages.filter((p) => p.kind === "doc").length} localized doc routes), synced frontend shaders, search indexes, sitemap, and LLM corpus.`,
);
