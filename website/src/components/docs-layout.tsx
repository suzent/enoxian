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

function HomeIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M3 10.5 12 3l9 7.5" />
      <path d="M5 9.5V20h14V9.5" />
    </svg>
  );
}

function QuickstartIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" y1="19" x2="20" y2="19" />
    </svg>
  );
}

function ArchitectureIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <polygon points="12 2 2 7 12 12 22 7 12 2" />
      <polyline points="2 12 12 17 22 12" />
      <polyline points="2 17 12 22 22 17" />
    </svg>
  );
}

function GitHubIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844a9.59 9.59 0 0 1 2.504.337c1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.02 10.02 0 0 0 22 12.017C22 6.484 17.522 2 12 2z" />
    </svg>
  );
}

function ReleaseIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}

function LlmCorpusIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
      <line x1="8" y1="13" x2="16" y2="13" />
      <line x1="8" y1="17" x2="13" y2="17" />
    </svg>
  );
}

function LanguageIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="m5 8 6 6" />
      <path d="m4 14 6-6 2-3" />
      <path d="M2 5h12" />
      <path d="M7 2h1" />
      <path d="m22 22-5-10-5 10" />
      <path d="M14 18h6" />
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

  const otherLocale = locale === "en" ? "zh-Hans" : "en";
  const switchUrl = localePath(currentUrl, otherLocale);
  const switchLabel = `${text.language} (${locale === "en" ? "中文" : "EN"})`;
  const releaseLabel = locale === "zh-Hans" ? "下载最新版本" : "Latest Release";
  const llmLabel =
    locale === "zh-Hans" ? "LLM 文档索引 (llms.txt)" : "LLM Docs (llms.txt)";

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
        {
          text: text.home,
          url: localePath("/", locale),
          active: "url",
          icon: <HomeIcon />,
        },
        {
          text: text.quickstart,
          url: localePath("/docs/guide/getting-started", locale),
          icon: <QuickstartIcon />,
        },
        {
          text: text.architecture,
          url: localePath("/docs/development/architecture/architecture", locale),
          icon: <ArchitectureIcon />,
        },
        {
          type: "icon",
          text: "GitHub",
          label: "GitHub",
          url: "https://github.com/suzent/enoxian",
          external: true,
          icon: (
            <span title="GitHub" className="inline-flex">
              <GitHubIcon />
            </span>
          ),
        },
        {
          type: "icon",
          text: releaseLabel,
          label: releaseLabel,
          url: "https://github.com/suzent/enoxian/releases/latest",
          external: true,
          icon: (
            <span title={releaseLabel} className="inline-flex">
              <ReleaseIcon />
            </span>
          ),
        },
        {
          type: "icon",
          text: "llms.txt",
          label: llmLabel,
          url: localePath("/llms.txt", locale),
          external: true,
          icon: (
            <span title={llmLabel} className="inline-flex">
              <LlmCorpusIcon />
            </span>
          ),
        },
        {
          type: "icon",
          text: switchLabel,
          label: switchLabel,
          url: switchUrl,
          active: "none",
          icon: (
            <span title={switchLabel} className="inline-flex">
              <LanguageIcon />
            </span>
          ),
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
            <Link aria-label={text.language} href={switchUrl}>
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
