import type { Locale } from "./locales";

const en = {
  docs: "Docs",
  quickstart: "Quick Start",
  architecture: "Architecture",
  home: "Home",
  search: "Search documentation",
  searchPlaceholder: "Search guides, CLI, protocols…",
  noResults: "No matching pages",
  searchLoading: "Loading index…",
  searchError: "Search unavailable.",
  retry: "Retry",
  handbook: "ENOXIAN DOCUMENTATION",
  tagline: "Peer-to-peer Circles",
  edit: "Edit on GitHub ↗",
  fallback:
    "Documentation is maintained in English; interface navigation is localized.",
  onThisPage: "On this page",
  notFound: "This page could not be found.",
  theme: "Theme",
  auto: "Auto",
  light: "Light",
  dark: "Dark",
  navigation: "Site navigation",
  language: "Switch language",
  copied: "Copied",
  copyFailed: "Copy failed",
  next: "Next page",
  previous: "Previous page",
  close: "Close",
  openSidebar: "Open navigation",
};

const zh: typeof en = {
  docs: "文档",
  quickstart: "快速开始",
  architecture: "架构",
  home: "首页",
  search: "搜索文档",
  searchPlaceholder: "搜索指南、CLI、协议…",
  noResults: "无匹配文档",
  searchLoading: "正在加载索引…",
  searchError: "搜索加载失败。",
  retry: "重试",
  handbook: "ENOXIAN 技术手册",
  tagline: "点对点共享 Circle",
  edit: "在 GitHub 编辑此页 ↗",
  fallback: "技术文档正文保持英文原文，界面导航已切换为中文。",
  onThisPage: "本页目录",
  notFound: "找不到该页面。",
  theme: "主题",
  auto: "跟随系统",
  light: "浅色",
  dark: "深色",
  navigation: "站点导航",
  language: "切换语言",
  copied: "已复制",
  copyFailed: "复制失败",
  next: "下一页",
  previous: "上一页",
  close: "关闭",
  openSidebar: "打开导航",
};

export function messages(locale: Locale): typeof en {
  return locale === "zh-Hans" ? zh : en;
}

export function fumadocsTranslations(locale: Locale): Record<string, string> {
  const text = messages(locale);
  return {
    Search: text.search,
    "On this page(table of contents)": text.onThisPage,
    "Next Page(pagination)": text.next,
    "Previous Page(pagination)": text.previous,
    "Copied Text(code block)(aria-label)": text.copied,
    "Copy Text(code block)(aria-label)":
      locale === "zh-Hans" ? "复制代码" : "Copy Text",
    "Copy Anchor Link(heading anchor)(aria-label)":
      locale === "zh-Hans" ? "复制锚点" : "Copy Anchor Link",
    "Open Sidebar(sidebar)(aria-label)": text.openSidebar,
    "Close Sidebar(sidebar)(aria-label)":
      locale === "zh-Hans" ? "关闭导航" : "Close Sidebar",
    "Collapse Sidebar(sidebar)(aria-label)":
      locale === "zh-Hans" ? "收起侧栏" : "Collapse Sidebar",
    "Toggle Theme(theme switcher)(aria-label)": text.theme,
    "Dark(theme switcher)(aria-label)": text.dark,
    "Light(theme switcher)(aria-label)": text.light,
    "System(theme switcher)(aria-label)": text.auto,
  };
}
