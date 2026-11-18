import { DocRedirect } from "@/components/doc-redirect";
import type { Metadata } from "next";
import Link from "next/link";
import { notFound } from "next/navigation";
import { getPage } from "@/lib/content";
import { localePath } from "@/lib/locales";
import { messages } from "@/lib/messages";
import Home from "@/components/landing/Home";
import { Document } from "@/components/document";
import { SiteNav } from "@/components/site-nav";
import { SiteFooter } from "@/components/site-footer";

type Props = { params: Promise<{ path?: string[] }> };

export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const page = getPage("/" + ((await params).path ?? []).join("/"));
  if (!page) return {};
  const isZh = page.locale === "zh-Hans";
  const canonical =
    page.redirectTo?.split("#")[0] ?? (page.url === "/" ? "/" : page.url + "/");

  return {
    title: page.kind === "home" ? { absolute: page.title } : page.title,
    description: page.description,
    alternates: {
      canonical,
      languages: {
        en: localePath(canonical, "en"),
        "zh-Hans": localePath(canonical, "zh-Hans"),
        "x-default": localePath(canonical, "en"),
      },
    },
    openGraph: {
      title: page.title,
      description: page.description,
      url: canonical,
      locale: isZh ? "zh_CN" : "en_US",
      type: "website",
    },
    twitter: {
      title: page.title,
      description: page.description,
    },
    ...(page.kind === "notfound" || page.kind === "redirect"
      ? { robots: { index: false, follow: true } }
      : {}),
  };
}

export default async function Page({ params }: Props) {
  const page = getPage("/" + ((await params).path ?? []).join("/"));
  if (!page) notFound();
  if (page.kind === "redirect") return <DocRedirect target={page.redirectTo!} />;
  const text = messages(page.locale);
  if (page.kind === "home") {
    return (
      <>
        <Home locale={page.locale} />
        <SiteFooter locale={page.locale} />
      </>
    );
  }
  if (page.kind === "doc" || page.kind === "category") {
    return <Document page={page} />;
  }
  return (
    <>
      <SiteNav />
      <main className="editorial">
        <h1>404</h1>
        <p>{text.notFound}</p>
        <Link href={localePath("/", page.locale)}>{text.home} →</Link>
      </main>
      <SiteFooter locale={page.locale} />
    </>
  );
}
