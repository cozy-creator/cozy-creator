import { useState } from "react"
import { Icon } from "./Icon"

/** The usual copy button, its icon alone (Paul, 11-16): `text` on the clipboard, and Copied once it's there. Its click selects, opens and
 *  drags nothing. */
export function CopyButton({ text, title = "Copy" }: { text: string; title?: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className="iconb copy canvas-control"
      aria-label={copied ? "Copied" : "Copy"}
      title={copied ? "Copied" : title}
      onClick={(event) => {
        event.stopPropagation()
        void navigator.clipboard.writeText(text).then(() => setCopied(true))
      }}
    >
      <Icon name={copied ? "check" : "copy"} />
    </button>
  )
}

/** Something to copy, shown: a code, an address, a login line. Never a link and never run, whoever said it. */
export function Copyable({ text }: { text: string }) {
  return (
    <span className="copyable">
      <code>{text}</code>
      <CopyButton text={text} />
    </span>
  )
}
