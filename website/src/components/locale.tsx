"use client";
import { usePathname } from "next/navigation";
import { pathLocale } from "@/lib/locales";
export { localePath, type Locale } from "@/lib/locales";

export function useLocale() {
  return pathLocale(usePathname());
}
