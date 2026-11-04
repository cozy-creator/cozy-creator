// One field drawn its own way: `shade`, as a row of swatches. No build step: React comes as a prop, and a sibling by a relative import.
import { steps } from "./steps.js";

export const api = 1;
export const field = "shade";

export default function Shade({ React, asset, fields, setFields, mentions, limits, tokens }) {
  const h = React.createElement;
  const [saved, setSaved] = React.useState("");
  const { min, max, step, default: fallback } = limits.shade;
  const held = fields.shade ?? fallback;
  const swatches = steps(min, max, step);
  const row = { display: "flex", gap: "4px", alignItems: "center", color: tokens.ink, fontFamily: tokens.mono };
  return h(
    "div",
    { "data-boxed": "shade", "data-asset": asset.name, "data-steps": swatches.join(","), "data-mentions": mentions.map((one) => one.name).join(","), "data-saved": saved, style: row },
    swatches.map((value) =>
      h(
        "button",
        {
          key: value,
          type: "button",
          "aria-label": `Shade ${value}`,
          "aria-pressed": value === held,
          style: { background: value === held ? tokens.accent : tokens.card, color: "inherit", border: `1px solid ${tokens.line}` },
          onClick: () => setFields({ shade: value }).then((stood) => setSaved(String(stood))),
        },
        String(value),
      ),
    ),
  );
}
