import type { ReactNode } from "react";
import Link from "next/link";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import type { Node } from "fumadocs-core/page-tree";
import { pages } from "@/lib/content";
import { localePath, type Locale } from "@/lib/locales";
import { messages } from "@/lib/messages";
import { Search } from "./search";

function EnoxianBrandMark() {
  return (
    <svg width="20" height="20" viewBox="0 0 512 512" aria-hidden="true">
      <defs>
        <mask id="enox-doc-cutout">
          <rect width="512" height="512" fill="white" />
          <circle cx="256" cy="288" r="52" fill="black" />
        </mask>
      </defs>
      <g mask="url(#enox-doc-cutout)">
        <polygon fill="currentColor" points="230,32 230,480 100,480 154,160" />
        <polygon fill="currentColor" points="282,32 282,480 412,480 358,160" />
      </g>
      <circle fill="currentColor" cx="256" cy="288" r="24" />
    </svg>
  );
}

export function DocumentationLayout({
  children,
  locale,
  currentUrl,
}: {
  children: ReactNode;
  locale: Locale;
  currentUrl: string;
}) {
  const text = messages(locale);
  const docs = pages.filter((p) => p.locale === locale && p.kind === "doc");
  const categories = pages.filter(
    (p) => p.locale === locale && p.kind === "category",
  );

  // Folders come from either synthetic categories OR doc pages that act as folder READMEs
  const folderEntries = [
    ...categories.map((c) => ({
      directory: c.directory!,
      title: c.title,
      url: c.url,
      position: c.position ?? 999,
      collapsed: !!c.collapsed,
    })),
    ...docs
      .filter((d) => d.isCategoryIndex && d.directory)
      .map((d) => ({
        directory: d.directory!,
        title: d.categoryTitle || d.title,
        url: d.url,
        position: d.position ?? 999,
        collapsed: !!d.collapsed,
      })),
  ];

  function parentDir(dir: string): string {
    return dir.split("/").slice(0, -1).join("/");
  }

  function childrenOf(directory: string): Node[] {
    const folderNodes = folderEntries
      .filter((f) => parentDir(f.directory) === directory)
      .map((folder) => ({
        position: folder.position,
        node: {
          type: "folder" as const,
          name: folder.title,
          ...(folder.collapsed ? { defaultOpen: false } : {}),
          index: {
            type: "page" as const,
            name: folder.title,
            url: folder.url,
          },
          children: childrenOf(folder.directory),
        },
      }));

    const docNodes = docs
      .filter((p) => {
        if (p.isCategoryIndex) return false;
        const dir = p.relative?.split("/").slice(0, -1).join("/") ?? "";
        return dir === directory;
      })
      .map((page) => ({
        position: page.position ?? 999,
        node: {
          type: "page" as const,
          name:
            page.relative === "index.md"
              ? locale === "zh-Hans"
                ? "总览"
                : "Overview"
              : page.title,
          url: page.url,
        },
      }));

    return [...folderNodes, ...docNodes]
      .sort(
        (a, b) =>
          a.position - b.position ||
          String(a.node.name).localeCompare(String(b.node.name)),
      )
      .map((item) => item.node);
  }

  return (
    <DocsLayout
      tree={{ name: text.docs, children: childrenOf("") }}
      nav={{
        title: (
          <span className="brand">
            <EnoxianBrandMark />
            ENOXIAN <small>/ {text.docs}</small>
          </span>
        ),
        url: localePath("/", locale),
      }}
      links={[
        { text: text.home, url: localePath("/", locale) },
        {
          text: text.quickstart,
          url: localePath("/docs/guide/getting-started", locale),
        },
        {
          text: "GitHub",
          url: "https://github.com/suzent/enoxian",
          external: true,
        },
      ]}
      searchToggle={{ enabled: false }}
      sidebar={{
        defaultOpenLevel: 1,
        banner: <Search />,
        footer: (
          <div className="sidebar-note">
            <span className="status-dot" />
            {text.tagline}
            <Link
              aria-label={text.language}
              href={localePath(currentUrl, locale === "en" ? "zh-Hans" : "en")}
            >
              {locale === "en" ? "中文" : "EN"}
            </Link>
          </div>
        ),
      }}
    >
      {children}
    </DocsLayout>
  );
}
