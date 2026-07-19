"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";

import { knowledgeSearchHref } from "../../lib/wiki-link";

export function legacyWikiDestination(search: string): string {
  const params = new URLSearchParams(search);
  const query = params.get("q")?.trim() || params.get("page")?.trim();
  return knowledgeSearchHref(query);
}

// `/web/wiki` is a pre-1.0 bookmark kept as a client-side redirect
// because the app builds with `output: "export"`.
function WikiRedirect() {
  const router = useRouter();
  const search = useSearchParams().toString();

  useEffect(() => {
    router.replace(legacyWikiDestination(search));
  }, [router, search]);

  return null;
}

export default function WikiRedirectPage() {
  return (
    <Suspense fallback={null}>
      <WikiRedirect />
    </Suspense>
  );
}
