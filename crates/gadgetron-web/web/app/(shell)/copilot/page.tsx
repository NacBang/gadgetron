"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

// `/web/copilot` merged into `/web` (ISSUE 47). The chat header's
// 모니터링 toggle renders the same chat-plus-MonitoringGrid split this
// route used to own — same runtime, same conversation, one less tab.
// Client-side replace (not a server `redirect()`) because the app
// builds with `output: "export"`. Kept so old bookmarks keep working.
export default function CopilotPage() {
  const router = useRouter();
  useEffect(() => {
    router.replace("/");
  }, [router]);
  return null;
}
