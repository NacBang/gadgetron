import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ConfirmProvider, useConfirm } from "../../app/components/ui/confirm";

// The promise-based confirm dialog (ISSUE 58) that replaced the
// scattered window.confirm() calls: resolves true on accept, false on
// cancel/dismiss, and surfaces the danger tone.

function Probe() {
  const confirm = useConfirm();
  return (
    <div>
      <button
        onClick={async () => {
          const ok = await confirm({
            title: "Delete user?",
            tone: "danger",
            confirmLabel: "Delete",
          });
          (document.getElementById("result") as HTMLElement).textContent = ok
            ? "accepted"
            : "rejected";
        }}
      >
        ask
      </button>
      <div id="result" data-testid="result" />
    </div>
  );
}

function setup() {
  render(
    <ConfirmProvider>
      <Probe />
    </ConfirmProvider>,
  );
}

describe("ConfirmProvider", () => {
  it("resolves true when the confirm button is clicked", async () => {
    setup();
    fireEvent.click(screen.getByText("ask"));
    expect(await screen.findByTestId("confirm-dialog")).toBeTruthy();
    // The danger tone + custom label render.
    const accept = screen.getByTestId("confirm-accept");
    expect(accept.textContent).toBe("Delete");
    fireEvent.click(accept);
    await waitFor(() =>
      expect(screen.getByTestId("result").textContent).toBe("accepted"),
    );
  });

  it("resolves false when cancelled", async () => {
    setup();
    fireEvent.click(screen.getByText("ask"));
    await screen.findByTestId("confirm-dialog");
    fireEvent.click(screen.getByTestId("confirm-cancel"));
    await waitFor(() =>
      expect(screen.getByTestId("result").textContent).toBe("rejected"),
    );
  });
});
