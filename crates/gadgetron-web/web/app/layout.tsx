import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Gadgetron",
  description: "AI assistant for your cluster",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ko">
      <head>
        <meta name="gadgetron-api-base" content="/v1" />
      </head>
      <body>{children}</body>
    </html>
  );
}
