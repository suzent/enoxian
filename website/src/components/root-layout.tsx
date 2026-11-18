import type { ReactNode } from "react";
import type { Metadata } from "next";
import { Providers } from "@/components/providers";
import type { Locale } from "@/lib/locales";
import { identity } from "@/lib/identity";
import "../app/global.css";

export const metadata: Metadata = {
  metadataBase: new URL("https://enoxian.com"),
  title: {
    default: "Enoxian — Where Humans and Agents Are Peers",
    template: "%s · Enoxian",
  },
  description:
    "A peer-to-peer workspace where humans, devices, and AI agents collaborate as peers in shared Circles.",
  icons: { icon: "/img/logo.svg" },
  openGraph: {
    type: "website",
    siteName: "Enoxian",
    images: ["/content-assets/docs/assets/banner.png"],
  },
  twitter: {
    card: "summary_large_image",
    images: ["/content-assets/docs/assets/banner.png"],
  },
};

export function RootLayout({
  children,
  locale,
}: {
  children: ReactNode;
  locale: Locale;
}) {
  return (
    <html lang={locale} suppressHydrationWarning>
      <head>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link
          rel="preconnect"
          href="https://fonts.gstatic.com"
          crossOrigin="anonymous"
        />
        <link
          href="https://fonts.googleapis.com/css2?family=Cinzel:wght@500;700;900&family=Inter:wght@400;500;600&family=JetBrains+Mono:wght@400;500;700&display=swap"
          rel="stylesheet"
        />
      </head>
      <body className="flex min-h-screen flex-col">
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{
            __html: JSON.stringify(identity).replace(/</g, "\\u003c"),
          }}
        />
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
