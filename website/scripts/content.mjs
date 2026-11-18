import fs from "node:fs";
import path from "node:path";
import matter from "gray-matter";
import GithubSlugger from "github-slugger";

export const site = "https://enoxian.com";
export const githubRepo = "https://github.com/suzent/enoxian";
export const root = path.resolve(import.meta.dirname, "..");
export const repo = path.resolve(root, "..");
export const docsRoot = path.join(repo, "docs");
export const locales = ["en", "zh-Hans"];

const toPosix = (p) => p.split(path.sep).join("/");

export const normalize = (url) => url.replace(/\/$/, "") || "/";
export const localize = (url, locale) =>
  locale === "en" ? url : `/zh-Hans${url === "/" ? "" : url}`;

const CATEGORY_META = {
  guide: {
    title: "Guides",
    zhTitle: "使用指南",
    position: 10,
    collapsed: false,
    description: "Install enoxian, create Circles, invite collaborators, and work with agents.",
  },
  concepts: {
    title: "Concepts",
    zhTitle: "核心概念",
    position: 20,
    collapsed: false,
    description: "Understand shared folders, proposals, security boundaries, and terminology.",
  },
  development: {
    title: "Development",
    zhTitle: "开发者文档",
    position: 30,
    collapsed: true,
    description: "Source builds, architecture, internal mechanics, and protocol references.",
  },
  "development/contributing": {
    title: "Contributing",
    zhTitle: "构建与贡献",
    position: 10,
    collapsed: false,
    description: "Local builds, checks, pull requests, and release procedures.",
  },
  "development/architecture": {
    title: "Architecture",
    zhTitle: "系统架构",
    position: 20,
    collapsed: false,
    description: "Daemon components, sync protocols, proposal snapshots, and security model.",
  },
  "development/reference": {
    title: "Reference",
    zhTitle: "协议与接口参考",
    position: 30,
    collapsed: true,
    description: "HTTP API, daemon config, P2P streams, invites, and agent adapters.",
  },
  "development/designs": {
    title: "Design Notes",
    zhTitle: "设计提案",
    position: 40,
    collapsed: true,
    description: "Proposed mechanisms, routing explorations, and open design questions.",
  },
};

const DOC_ORDER = {
  "index.md": 1,
  "guide/getting-started.md": 10,
  "guide/collaboration.md": 20,
  "guide/invite.md": 30,
  "guide/link.md": 40,
  "guide/agents.md": 50,
  "guide/cli.md": 60,
  "guide/rendezvous-setup.md": 70,
  "concepts/overview.md": 10,
  "concepts/proposals.md": 20,
  "concepts/security.md": 30,
  "concepts/concepts.md": 40,
  "development/contributing/dev-guide.md": 10,
  "development/contributing/releasing.md": 20,
  "development/architecture/architecture.md": 10,
  "development/architecture/internals.md": 20,
  "development/architecture/storage.md": 30,
  "development/architecture/proposals.md": 40,
  "development/architecture/security.md": 50,
  "development/reference/api.md": 10,
  "development/reference/daemon.md": 20,
  "development/reference/protocol.md": 30,
  "development/reference/p2p-protocols.md": 40,
  "development/reference/invites.md": 50,
  "development/reference/device-linking.md": 60,
  "development/reference/execution-inbox.md": 70,
  "development/reference/agent-integration.md": 80,
};

export function discover(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      if (entry.name === "assets" || entry.name === "paper" || entry.name.startsWith(".")) {
        return [];
      }
      const full = path.join(dir, entry.name);
      return entry.isDirectory()
        ? discover(full)
        : /\.mdx?$/.test(entry.name)
          ? [full]
          : [];
    });
}

export function docUrl(relative, slug) {
  const rel = toPosix(relative);
  if (slug) return normalize("/docs/" + String(slug).replace(/^\//, ""));
  const parts = rel
    .replace(/\.mdx?$/, "")
    .split("/")
    .map((p) => p.replace(/^\d+-/, ""));
  const last = parts.at(-1);
  // Collapse README.md / index.md to parent directory, while keeping e.g. concepts/concepts.md distinct
  if (/^(readme|index)$/i.test(last)) parts.pop();
  return normalize("/docs/" + parts.filter(Boolean).join("/"));
}

function read(file) {
  const { data, content } = matter(fs.readFileSync(file, "utf8"));
  const title =
    data.title ||
    content.match(/^# (.+)$/m)?.[1]?.trim() ||
    path.basename(file, path.extname(file));
  return { data, content, title };
}

function outsideFences(text, transform) {
  let fence = "";
  return text
    .split(/\r?\n/)
    .map((line) => {
      const match = line.match(/^\s*(`{3,}|~{3,})/);
      if (match) {
        if (!fence) fence = match[1];
        else if (fence[0] === match[1][0] && match[1].length >= fence.length)
          fence = "";
        return line;
      }
      return fence ? line : transform(line);
    })
    .join("\n");
}

/**
 * All documentation in ../docs/ is standard CommonMark compiled with format: "md" in MDXRemote.
 */

export function tableOfContents(content) {
  const slugger = new GithubSlugger();
  const toc = [];
  outsideFences(content, (line) => {
    const h = line.match(/^(#{2,6}) (.+)$/);
    if (h) {
      const title = h[2]
        .replace(/`([^`]+)`/g, "$1")
        .replace(/\*\*([^*]+)\*\*/g, "$1")
        .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
        .trim();
      toc.push({
        title,
        url: "#" + slugger.slug(title),
        depth: h[1].length,
      });
    }
    return line;
  });
  return toc;
}

export function createContent() {
  const files = discover(docsRoot);
  const rawDocs = files
    .map((file) => {
      const { data, content, title } = read(file);
      const relative = toPosix(path.relative(docsRoot, file));
      const isFolderIndex = /^(?:.+\/)?(?:readme|index)\.mdx?$/i.test(relative) && relative !== "index.md";
      const folderDir = isFolderIndex ? toPosix(path.dirname(relative)) : undefined;
      return {
        kind: "doc",
        url: docUrl(relative, data.slug),
        file,
        source: toPosix(path.relative(repo, file)),
        relative,
        folderDir,
        title,
        content,
        description: data.description || "",
        position: data.sidebar_position ?? DOC_ORDER[relative] ?? 999,
        draft: !!data.draft,
      };
    })
    .filter((doc) => !doc.draft);

  const docsByFile = new Map(rawDocs.map((doc) => [path.resolve(doc.file), doc]));
  const docsByUrl = new Map(rawDocs.map((doc) => [doc.url, doc]));

  // Build category nodes for directories that do not already have a README.md/index.md doc at their URL,
  // or attach metadata to folder index docs so sidebar trees render nested folders cleanly.
  const categories = [];
  const seenDirs = new Set();
  for (const doc of rawDocs) {
    let dir = path.dirname(doc.file);
    while (dir !== docsRoot && dir.startsWith(docsRoot)) {
      const relDir = toPosix(path.relative(docsRoot, dir));
      if (!seenDirs.has(relDir)) {
        seenDirs.add(relDir);
        const meta = CATEGORY_META[relDir] || {};
        const catUrl = normalize("/docs/" + relDir);
        const existingIndex = docsByUrl.get(catUrl);
        const title =
          meta.title ||
          path.basename(dir).replace(/^\d+-/, "").replaceAll("-", " ");
        if (existingIndex) {
          existingIndex.isCategoryIndex = true;
          existingIndex.directory = relDir;
          existingIndex.categoryTitle = title;
          existingIndex.zhCategoryTitle = meta.zhTitle || title;
          existingIndex.position = meta.position ?? existingIndex.position;
          existingIndex.collapsed = meta.collapsed ?? false;
          if (!existingIndex.description && meta.description) {
            existingIndex.description = meta.description;
          }
        } else {
          categories.push({
            kind: "category",
            url: catUrl,
            title,
            zhTitle: meta.zhTitle || title,
            description: meta.description || "",
            directory: relDir,
            position: meta.position ?? 999,
            collapsed: !!meta.collapsed,
          });
        }
      }
      dir = path.dirname(dir);
    }
  }

  const other = [
    {
      kind: "home",
      url: "/",
      title: "Enoxian — Where Humans and Agents Are Peers",
      description:
        "A peer-to-peer workspace where humans, devices, and AI agents collaborate as peers in shared Circles.",
    },
    { kind: "notfound", url: "/404", title: "Page not found" },
  ];

  const redirects = [
    {
      kind: "redirect",
      url: "/guide",
      redirectTo: "/docs/guide/getting-started",
      title: "Page moved",
    },
  ];

  const all = [...rawDocs, ...categories, ...other, ...redirects];
  const urls = new Set(all.map((p) => p.url));
  const unresolved = [];

  function resolveTarget(target, doc, isImage = false) {
    if (/^(https?:|mailto:|tel:|data:|#)/i.test(target)) {
      if (target.startsWith(site + "/")) {
        const u = new URL(target);
        if (urls.has(normalize(u.pathname))) {
          return localize(normalize(u.pathname), doc.locale) + u.search + u.hash;
        }
      }
      return target;
    }
    const match = target.match(/^([^?#]*)(.*)$/);
    const raw = decodeURI(match[1]);
    const suffix = match[2];
    if (raw.startsWith("/zh-Hans/")) return target;
    if (raw.startsWith("/")) {
      if (urls.has(normalize(raw))) {
        return localize(normalize(raw), doc.locale) + suffix;
      }
      if (raw.startsWith("/img/") || raw.startsWith("/assets/") || raw.startsWith("/content-assets/")) {
        return target;
      }
    }

    const sourceFile = path.resolve(path.dirname(doc.file), raw);
    if (isImage && fs.existsSync(sourceFile)) {
      const rel = toPosix(path.relative(repo, sourceFile));
      if (rel.startsWith("..")) {
        throw new Error("Image outside repository: " + sourceFile);
      }
      return "/content-assets/" + rel + suffix;
    }

    const candidate =
      docsByFile.get(sourceFile) ||
      docsByFile.get(path.resolve(sourceFile + ".md")) ||
      docsByFile.get(path.join(sourceFile, "README.md")) ||
      docsByFile.get(path.join(sourceFile, "index.md"));
    if (candidate) return localize(candidate.url, doc.locale) + suffix;

    if (fs.existsSync(sourceFile) && toPosix(sourceFile).startsWith(toPosix(repo) + "/")) {
      return (
        githubRepo +
        "/blob/main/" +
        toPosix(path.relative(repo, sourceFile)) +
        suffix
      );
    }

    unresolved.push({ source: doc.source, target });
    return target;
  }

  const pages = [];
  for (const locale of locales) {
    for (const original of all) {
      const doc = {
        ...original,
        locale,
        canonicalPath: original.url,
        url: localize(original.url, locale),
        translated: locale === "en",
      };

      if (locale === "zh-Hans") {
        if (original.kind === "home") {
          doc.title = "Enoxian — 人与智能体对等协作的点对点工作区";
          doc.description =
            "去中心化、点对点的协作空间：将共享笔记、对话、任务与异构 AI 智能体汇聚于同一个 Circle。";
          doc.translated = true;
        } else if (original.kind === "category" && original.zhTitle) {
          doc.title = original.zhTitle;
          doc.translated = true;
        } else if (original.isCategoryIndex && original.zhCategoryTitle) {
          doc.categoryTitle = original.zhCategoryTitle;
        }
      }

      if (doc.redirectTo) doc.redirectTo = localize(doc.redirectTo, locale);

      if (doc.content) {
        const sourceDoc = { ...doc, url: original.url };
        let removedTitle = false;
        doc.content = outsideFences(doc.content, (line) => {
          if (!removedTitle && /^# /.test(line)) {
            removedTitle = true;
            return "";
          }
          const rewritten = line.replace(
            /(!?\[[^\]]*\])\(<?([^\s)<>]+)>?(\s+"[^"]*")?\)/g,
            (_, label, target, title = "") =>
              `${label}(${resolveTarget(target, sourceDoc, label.startsWith("!"))}${title})`,
          );
          return rewritten;
        });

        doc.toc = tableOfContents(doc.content);
        doc.searchText = doc.content
          .replace(/```[^\n]*\n/g, "\n")
          .replace(/<[^>]+>/g, " ")
          .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
          .replace(/[#*`|\\]/g, " ")
          .replace(/\s+/g, " ")
          .trim();
        if (!doc.description) doc.description = doc.searchText.slice(0, 180);
      }

      delete doc.file;
      pages.push(doc);
    }
  }

  if (unresolved.length) {
    throw new Error(
      "Unresolved documentation links:\n" + JSON.stringify(unresolved, null, 2),
    );
  }
  const duplicates = pages.filter(
    (p, i) => pages.findIndex((other) => other.url === p.url) !== i,
  );
  if (duplicates.length) {
    throw new Error("Duplicate routes: " + duplicates.map((p) => p.url));
  }

  return { pages };
}
