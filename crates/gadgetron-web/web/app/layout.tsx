import type { Metadata } from "next";
import "./globals.css";
import { Geist } from "next/font/google";
import { cn } from "@/lib/utils";

const geist = Geist({subsets:['latin'],variable:'--font-sans'});

export const metadata: Metadata = {
  title: "Gadgetron",
  description: "AI assistant for your cluster",
  // ManyCoreSoft "M" mark cropped from /public/brand/manycoresoft.png.
  // Dark-square background variants (cooked into the PNGs at generate
  // time) so the white M stays visible on light browser-tab chrome —
  // pure-transparent icons disappear into the tab background on most
  // light themes.
  icons: {
    icon: [
      { url: "/web/favicon.ico", sizes: "any" },
      { url: "/web/icon-16.png", type: "image/png", sizes: "16x16" },
      { url: "/web/icon-32.png", type: "image/png", sizes: "32x32" },
      { url: "/web/icon-192.png", type: "image/png", sizes: "192x192" },
      { url: "/web/icon-512.png", type: "image/png", sizes: "512x512" },
    ],
    apple: { url: "/web/apple-icon.png", sizes: "180x180" },
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ko" className={cn("dark font-sans", geist.variable)}>
      <head>
        <meta name="gadgetron-api-base" content="/v1" />
      </head>
      <body>{children}</body>
    </html>
  );
}
