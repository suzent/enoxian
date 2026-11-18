"use client";
import { useState, useEffect } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useTheme } from "next-themes";
import { useLocale, localePath } from "./locale";
import { messages } from "@/lib/messages";

function EnoxianLogo() {
  return (
    <svg width="20" height="20" viewBox="0 0 512 512" aria-hidden="true">
      <defs>
        <mask id="enox-nav-cutout">
          <rect width="512" height="512" fill="white" />
          <circle cx="256" cy="288" r="52" fill="black" />
        </mask>
      </defs>
      <g mask="url(#enox-nav-cutout)">
        <polygon fill="currentColor" points="230,32 230,480 100,480 154,160" />
        <polygon fill="currentColor" points="282,32 282,480 412,480 358,160" />
      </g>
      <circle fill="currentColor" cx="256" cy="288" r="24" />
    </svg>
  );
}

function MoonIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}

function SunIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="4" />
      <line x1="12" y1="2" x2="12" y2="5" />
      <line x1="12" y1="19" x2="12" y2="22" />
      <line x1="4.22" y1="4.22" x2="6.34" y2="6.34" />
      <line x1="17.66" y1="17.66" x2="19.78" y2="19.78" />
      <line x1="2" y1="12" x2="5" y2="12" />
      <line x1="19" y1="12" x2="22" y2="12" />
      <line x1="4.22" y1="19.78" x2="6.34" y2="17.66" />
      <line x1="17.66" y1="6.34" x2="19.78" y2="4.22" />
    </svg>
  );
}

function AutoIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3v18" />
      <path d="M12 7a5 5 0 0 1 0 10" fill="currentColor" stroke="none" />
    </svg>
  );
}

function GitHubIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0 1 12 6.844a9.59 9.59 0 0 1 2.504.337c1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.02 10.02 0 0 0 22 12.017C22 6.484 17.522 2 12 2z" />
    </svg>
  );
}

export function SiteNav() {
  const { theme, setTheme } = useTheme();
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  const choice = mounted && theme !== "system" ? theme : null;
  const locale = useLocale();
  const text = messages(locale);
  const pathname = usePathname();

  const otherLocale = locale === "en" ? "zh-Hans" : "en";
  const otherLabel = otherLocale === "zh-Hans" ? "中文" : "EN";
  const stripped = pathname.replace(/^\/(zh-Hans)(\/|$)/, "/") || "/";
  const switchHref =
    otherLocale === "en"
      ? stripped
      : `/zh-Hans${stripped === "/" ? "" : stripped}`;

  function cycleTheme() {
    if (choice === null) setTheme("light");
    else if (choice === "light") setTheme("dark");
    else setTheme("system");
  }

  const themeIcon =
    choice === "light" ? <SunIcon /> : choice === "dark" ? <MoonIcon /> : <AutoIcon />;
  const themeLabel =
    choice === "light" ? text.light : choice === "dark" ? text.dark : text.auto;

  return (
    <header className="enox-nav" aria-label={text.navigation}>
      <div className="enox-nav-inner">
        <Link href={localePath("/", locale)} className="enox-nav-brand">
          <EnoxianLogo />
          <span className="enox-nav-title">ENOXIAN</span>
        </Link>
        <div className="enox-nav-links">
          <Link href={localePath("/docs", locale)} className="enox-nav-link">
            {text.docs}
          </Link>
          <Link
            href={localePath("/docs/guide/getting-started", locale)}
            className="enox-nav-link"
          >
            {text.quickstart}
          </Link>
          <Link
            href={localePath("/docs/development/architecture/architecture", locale)}
            className="enox-nav-link enox-hide-mobile"
          >
            {text.architecture}
          </Link>
          <div className="enox-nav-utils">
            <a
              href="https://github.com/suzent/enoxian"
              className="enox-nav-btn"
              target="_blank"
              rel="noopener noreferrer"
              aria-label="GitHub"
            >
              <GitHubIcon />
            </a>
            <Link
              href={switchHref}
              className="enox-nav-btn"
              aria-label={text.language}
            >
              {otherLabel}
            </Link>
            <button
              type="button"
              className="enox-nav-btn"
              onClick={cycleTheme}
              aria-label={`${text.theme}: ${themeLabel}`}
              title={themeLabel}
            >
              {themeIcon}
            </button>
          </div>
        </div>
      </div>
    </header>
  );
}
