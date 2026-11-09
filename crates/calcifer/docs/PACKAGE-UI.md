# A package's own UI

A package's declared fields are the normal path, and no first-party package uses a component: this is secondary, there to explore.

A package that wants more ships a component: a real ES module in the page, with no sandbox and no prompt. Installing a package is the trust decision, as its code already runs on that machine.

```python
@app.entrypoint(node=Node(ui="ui/box.js"))
def generate(payload: Input, out: Outputs) -> Output: ...
```

`ui` is a literal, read statically, naming a file beside the module that declares the function, in its wheel. A build must include it: `uv_build` and hatchling do, and setuptools needs `package-data`. `cozy package publish` warns of one the wheel doesn't hold.

## The API

```js
export const api = 1;
export const field = "shade"; // optional: the one field it draws; absent, the whole edit face

export default function Box({ React, asset, fields, setFields, mentions, limits, tokens }) {
  const h = React.createElement;
  return h("button", { onClick: () => setFields({ shade: 1 }) }, String(fields.shade));
}
```

| Prop | What it is |
|---|---|
| `React` | The app's React. Bundle none: another copy's hooks don't work here. With JSX, set your compiler's factory to `React.createElement`. |
| `asset` | `{name, kind, tool, standing, versions}` |
| `fields` | What it holds now, what's typed included. |
| `setFields(fields)` | Writes these to the project file. Resolves to whether that stood. |
| `mentions` | `[{name, kind}]`: what its text can name as `<name>`. |
| `limits` | What the tool declares of each field, by field: its `Tool.fields` entry (`CALCIFER-API.md`), `{type, label, required?, default?, auto?, placeholder?, …}` by type. |
| `tokens` | The host's look, as CSS values: `bg card panel line ink mute faint accent good warn busy bad sans mono`. |

A component for one field is drawn where that field's control would be: behind the box's More, unless it's the box's first text, its beats or required. A whole face is drawn in place of them all, and More then holds only what the app adds.

`api` is what a module speaks. One that says anything else isn't drawn. Neither is one that throws, or can't be got: the declared box stands, and the canvas goes on. The approved face is always the app's.

It runs with your power in the page, under its CSP, so no inline `<style>` and no `<script>` from elsewhere: use the `style` prop. Its siblings are real modules too: a relative import reaches any `.js` or `.mjs` under the folder `ui` names. A `ui` at the top of a package is that one file.

## Where the bytes come from

Only from this computer's Calcifer (`GET /v1/tools/{id}/ui`, `CALCIFER-API.md`), which reads no code of it and never asks a pod:

- **A release:** the newest any machine you hold has installed, from the Hub, its wheel held to the digest the Hub says. A wheel that isn't doesn't load.
- **Local code a project names:** its files as they are now. An edit is another hash, so another URL.
