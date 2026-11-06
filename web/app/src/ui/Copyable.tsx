import { useState } from "react"

/** A command, as text to copy. Never a link and never run, whoever said it. */
export function Copyable({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <span className="copyable">
      <code>{text}</code>
      <button
        type="button"
        onClick={() => {
          void navigator.clipboard.writeText(text).then(() => setCopied(true))
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </span>
  )
}
