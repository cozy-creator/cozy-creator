// The whole edit face: its text, typed in its own box.
export const api = 1;

export default function Whole({ React, fields, setFields }) {
  const h = React.createElement;
  const [typed, setTyped] = React.useState(fields.instructions ?? "");
  return h("input", {
    "data-boxed": "whole",
    "aria-label": "Its own words",
    value: typed,
    onChange: (event) => setTyped(event.target.value),
    onBlur: () => setFields({ instructions: typed }),
  });
}
