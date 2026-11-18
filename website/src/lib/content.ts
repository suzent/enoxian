import data from "../../.generated/content.json";
import type { Locale } from "./locales";

export type PageKind =
  | "redirect"
  | "doc"
  | "category"
  | "home"
  | "notfound";

export interface Page {
  kind: PageKind;
  url: string;
  canonicalPath: string;
  title: string;
  categoryTitle?: string;
  description?: string;
  redirectTo?: string;
  locale: Locale;
  content?: string;
  source?: string;
  translated: boolean;
  relative?: string;
  directory?: string;
  isCategoryIndex?: boolean;
  position?: number;
  collapsed?: boolean;
  toc?: { title: string; url: string; depth: number }[];
  searchText?: string;
}

export const pages = data.pages as Page[];

export function getPage(url: string): Page | undefined {
  return pages.find((page) => page.url === (url.replace(/\/$/, "") || "/"));
}
