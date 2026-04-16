import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "export",
  // Assets served from /web/ when embedded in gadgetron binary
  basePath: "/web",
  images: { unoptimized: true },
};

export default nextConfig;
