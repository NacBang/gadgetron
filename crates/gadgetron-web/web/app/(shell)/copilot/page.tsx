"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

// `/web/copilot` is a pre-1.0 bookmark kept as a client-side redirect
// because the app builds with `output: "export"`.
export default function CopilotPage() {
  const router = useRouter();
  useEffect(() => {
    router.replace("/");
  }, [router]);
  return null;
}
