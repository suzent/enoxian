import Link from "next/link";
import { localePath, type Locale } from "@/lib/locales";
import { messages } from "@/lib/messages";

export function SiteFooter({ locale }: { locale: Locale }) {
  const text = messages(locale);
  return (
    <footer className="site-footer">
      <span>© {new Date().getFullYear()} ENOXIAN · MIT LICENSE</span>
      <Link href={localePath("/docs", locale)}>{text.docs}</Link>
      <Link href={localePath("/docs/concepts/security", locale)}>
        {locale === "zh-Hans" ? "隐私与安全" : "Security"}
      </Link>
      <a
        href="https://github.com/suzent/enoxian/releases/latest"
        target="_blank"
        rel="noopener noreferrer"
      >
        Releases ↗
      </a>
      <a
        href="https://github.com/suzent/enoxian"
        target="_blank"
        rel="noopener noreferrer"
      >
        GitHub ↗
      </a>
    </footer>
  );
}
