import { describe, expect, it } from "vitest";

import {
  MAX_LOCAL_BUNDLE_SOURCE_BYTES,
  localBundleSourceSizeError,
} from "../../app/components/admin/bundle-control-plane";

describe("local Bundle package source", () => {
  it("matches the dedicated 16 MiB server route instead of the generic JSON limit", () => {
    expect(MAX_LOCAL_BUNDLE_SOURCE_BYTES).toBe(16 * 1024 * 1024);
    expect(localBundleSourceSizeError(5 * 1024 * 1024)).toBeNull();
    expect(localBundleSourceSizeError(MAX_LOCAL_BUNDLE_SOURCE_BYTES + 1)).toBe(
      "Bundle package must be 16 MiB or smaller",
    );
  });
});
