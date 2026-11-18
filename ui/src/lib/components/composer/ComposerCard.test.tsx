import { render } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import ComposerCard from "./ComposerCard.tsx";

let aboveRenders = 0;
let selectorRenders = 0;
let actionRenders = 0;

function Slot(props: { name: string; onRender: () => void }) {
  props.onRender();
  return <span data-testid={`slot-${props.name}`}>{props.name}</span>;
}

describe("ComposerCard", () => {
  it("evaluates each JSX slot once", () => {
    aboveRenders = 0;
    selectorRenders = 0;
    actionRenders = 0;

    const { container } = render(() => (
      <ComposerCard
        above={<Slot name="above" onRender={() => aboveRenders++} />}
        selectors={<Slot name="selectors" onRender={() => selectorRenders++} />}
        actions={<Slot name="actions" onRender={() => actionRenders++} />}
      >
        <div data-testid="input" />
      </ComposerCard>
    ));

    // Reading a JSX prop in `Show when` (or in an `!== undefined` check) would
    // build a second, detached tree and run its component body again.
    expect(aboveRenders).toBe(1);
    expect(selectorRenders).toBe(1);
    expect(actionRenders).toBe(1);
    expect(container.querySelectorAll('[data-testid="slot-above"]')).toHaveLength(1);
    expect(container.querySelectorAll('[data-testid="slot-selectors"]')).toHaveLength(1);
    expect(container.querySelectorAll('[data-testid="slot-actions"]')).toHaveLength(1);
  });
});
