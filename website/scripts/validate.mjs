import fs from "node:fs";
import path from "node:path";
import {
  createContent,
  root,
  site,
  normalize,
  localize,
  locales,
} from "./content.mjs";

const { pages } = createContent();
const output = path.join(root, "out");
const errors = [];

const decode = (value) =>
  value
    .replaceAll("&amp;", "&")
    .replaceAll("&quot;", '"')
    .replaceAll("&#x27;", "'")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">");

const htmlByUrl = new Map();
for (const page of pages) {
  const relSegments = page.url.split("/").filter(Boolean);
  const file = path.join(output, ...relSegments, "index.html");
  if (!fs.existsSync(file)) {
    errors.push("Missing HTML: " + page.url);
    continue;
  }
  const html = fs.readFileSync(file, "utf8");
  htmlByUrl.set(page.url, html);
  if (!html.includes("<h1")) errors.push("Missing page heading: " + page.url);
  if (!html.includes(`lang="${page.locale}"`))
    errors.push("Missing locale attribute: " + page.url);
  if (!html.includes('rel="canonical"'))
    errors.push("Missing canonical metadata: " + page.url);
}

for (const [url, html] of htmlByUrl) {
  for (const match of html.matchAll(
    /<(a|img|script|link)\b[^>]*?\b(href|src)="([^"]+)"/g,
  )) {
    const target = decode(match[3]);
    if (!target || /^(mailto:|tel:|data:|javascript:)/.test(target)) continue;
    const resolved = new URL(
      target,
      site + (url === "/" ? "/" : url + "/"),
    );
    if (resolved.origin !== site) continue;
    const destination = normalize(decodeURI(resolved.pathname));
    if (htmlByUrl.has(destination)) {
      if (resolved.hash) {
        const id = decodeURIComponent(resolved.hash.slice(1));
        const targetHtml = htmlByUrl.get(destination);
        const ids = [...targetHtml.matchAll(/\bid="([^"]+)"/g)].map((m) =>
          decode(m[1]),
        );
        if (!ids.includes(id)) errors.push(`${url}: missing anchor ${target}`);
      }
    } else if (
      !fs.existsSync(
        path.join(output, ...decodeURI(resolved.pathname).split("/").filter(Boolean)),
      )
    ) {
      errors.push(`${url}: missing target ${target}`);
    }
  }
}

for (const requiredAsset of [".nojekyll", "CNAME", "sitemap.xml", "robots.txt", "img/logo.svg"]) {
  if (!fs.existsSync(path.join(output, ...requiredAsset.split("/")))) {
    errors.push("Missing root static asset: " + requiredAsset);
  }
}

for (const locale of locales) {
  const prefix = locale === "en" ? "" : locale + "/";
  for (const file of ["search-index.json", "llms.txt", "llms-full.txt"]) {
    if (!fs.existsSync(path.join(output, ...(prefix + file).split("/")))) {
      errors.push("Missing generated locale asset: " + prefix + file);
    }
  }
  const home = htmlByUrl.get(localize("/", locale));
  if (!home || !home.includes("install.sh")) {
    errors.push("Missing install command on landing page: " + locale);
  }
  if (locale === "zh-Hans" && (!home || !home.includes("对等协作"))) {
    errors.push("Missing Chinese landing copy");
  }
  const quickstart = htmlByUrl.get(localize("/docs/guide/getting-started", locale));
  if (!quickstart || !quickstart.includes("Create a Circle")) {
    errors.push("Missing getting-started doc content: " + locale);
  }
}

if (errors.length) {
  console.error([...new Set(errors)].join("\n"));
  process.exit(1);
}

console.log(
  `Validated ${pages.length} static HTML routes, internal links, heading anchors, CNAME, search indexes, and LLM corpus.`,
);
