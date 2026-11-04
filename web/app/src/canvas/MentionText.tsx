import { type ClipboardEvent, type KeyboardEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useSignal } from "../ui/signal"
import { useCanvasStore, useElement } from "./context"
import { Thumb } from "./Media"
import { shownOf } from "./shown"
import { mentionsIn, withMentions } from "./text"

function chip(name: string, missing: boolean): HTMLElement {
  const el = document.createElement("span")
  el.className = missing ? "chip missing" : "chip"
  el.contentEditable = "false"
  el.dataset.mention = name
  el.textContent = name
  el.title = missing ? `${name} is no longer here` : name
  return el
}

/** What the box says, as the file will hold it: plain text, `<name>` for each chip. */
function read(box: HTMLElement): string {
  let text = ""
  const walk = (parent: globalThis.Node) => {
    for (const child of parent.childNodes) {
      if (child instanceof Text) text += child.data
      else if (child instanceof HTMLElement && child.dataset.mention) text += `<${child.dataset.mention}>`
      else if (child instanceof HTMLBRElement) text += child === box.lastChild ? "" : "\n"
      else walk(child)
    }
  }
  walk(box)
  return text
}

interface Menu {
  options: string[]
  chosen: number
  x: number
  y: number
  /** What it replaces, the `<` and what's typed after it. */
  range: Range
}

/**
 * Text with each `<name>` shown as a chip, and the file still holding plain text. Built with the DOM's text calls, so what anyone wrote
 * is never HTML. A chip means a name of this project, or a missing one when Calcifer says so; anything else in `<>` stays words.
 */
export function MentionText({
  asset,
  place,
  value,
  label,
  missing,
  focus,
  onDraft,
  onSave,
}: {
  asset: string
  place: string
  value: string
  label: string
  missing: string[]
  focus?: boolean
  onDraft: (text: string) => void
  onSave: () => void
}) {
  const store = useCanvasStore()
  const box = useRef<HTMLDivElement>(null)
  const caret = useRef<Range | null>(null)
  const [menu, setMenu] = useState<Menu | null>(null)
  const names = useSignal(store.names)

  // While it's being typed in, what it holds is what's typed. Left alone it shows names as they now stand, painted only if that differs,
  // so its caret survives a blur and a drop can still land there.
  // biome-ignore lint/correctness/useExhaustiveDependencies: names tells it a chip may have come or gone
  useLayoutEffect(() => {
    const el = box.current
    // What ends a text is its file's to say: what's typed, and the caret in it, stand.
    if (!el || (document.activeElement === el && read(el).trimEnd() === value.trimEnd())) return
    const nodes = withMentions(value).map((part) => {
      const inner = part.text.slice(1, -1)
      if (part.mention && missing.includes(inner)) return chip(inner, true)
      if (part.mention && store.nameOf(inner)) return chip(inner, false)
      return document.createTextNode(part.text)
    })
    const merged = (held: ChildNode[]) => held.reduce((said, node) => said + (node instanceof HTMLElement ? `\u0000${node.className}:${node.dataset.mention}\u0000` : node.textContent), "")
    const trimmed = (said: string) => said.replace(/\s+$/, "")
    if (trimmed(merged([...el.childNodes])) === trimmed(merged(nodes))) return
    el.replaceChildren(...nodes)
    // What it pointed into is gone: a name dropped now goes at the end.
    caret.current = null
  }, [value, missing, store, names])

  const emit = useCallback(() => {
    if (box.current) onDraft(read(box.current))
  }, [onDraft])

  const remember = useCallback(() => {
    const selection = document.getSelection()
    const at = selection?.anchorNode
    if (!selection?.rangeCount || !at || !box.current?.contains(at)) return
    // A caret in a name is in no text: a click on one, or End with one last, and what's typed would go nowhere. It goes just after.
    const name = selection.isCollapsed ? (at instanceof Element ? at : at.parentElement)?.closest("[data-mention]") : null
    if (name?.parentNode) return selection.collapse(name.parentNode, [...name.parentNode.childNodes].indexOf(name) + 1)
    caret.current = selection.getRangeAt(0).cloneRange()
  }, [])

  /** A chip where `at` says, or else at the end, with a space after it to go on typing. */
  const put = useCallback(
    (name: string, at: Range | null) => {
      const el = box.current
      if (!el) return
      const made = chip(name, false)
      const after = document.createTextNode(" ")
      if (at && el.contains(at.startContainer)) {
        at.deleteContents()
        at.insertNode(after)
        at.insertNode(made)
      } else {
        if (el.textContent && !/\s$/.test(el.textContent)) el.append(" ")
        el.append(made, after)
      }
      if (document.activeElement === el) {
        const selection = document.getSelection()
        selection?.collapse(after, 1)
      }
      emit()
    },
    [emit],
  )

  // A drop, or anything else asking this text to take a name, goes where its caret last was.
  useEffect(() => store.openCaret(asset, place, (name) => put(name, caret.current)), [store, asset, place, put])

  useEffect(() => {
    const el = box.current
    if (!el) return
    store.dropTargets.set(el, {
      over: () => {},
      drop: (names) => {
        for (const name of names) if (name !== asset) put(name, caret.current)
        onSave()
      },
    })
    return () => void store.dropTargets.delete(el)
  }, [store, asset, put, onSave])

  useEffect(() => {
    if (focus) box.current?.focus()
  }, [focus])

  useEffect(() => {
    document.addEventListener("selectionchange", remember)
    return () => document.removeEventListener("selectionchange", remember)
  }, [remember])

  /** After each keystroke: `<name>` finished by hand is a chip, and a `<` being typed offers names. */
  const look = () => {
    const selection = document.getSelection()
    const at = selection?.anchorNode
    if (!selection?.isCollapsed || !(at instanceof Text) || !box.current?.contains(at)) return setMenu(null)
    const before = at.data.slice(0, selection.anchorOffset)
    const closed = before.match(/<([^<>\s]+)>$/)
    if (closed && store.nameOf(closed[1])) {
      const range = document.createRange()
      range.setStart(at, selection.anchorOffset - closed[0].length)
      range.setEnd(at, selection.anchorOffset)
      setMenu(null)
      return put(closed[1], range)
    }
    const open = before.match(/<([^<>\s]*)$/)
    const options = open ? store.namesLike(open[1], asset).slice(0, 8) : []
    if (!open || !options.length) return setMenu(null)
    const range = document.createRange()
    range.setStart(at, selection.anchorOffset - open[0].length)
    range.setEnd(at, selection.anchorOffset)
    const shown = range.getBoundingClientRect()
    setMenu({ options, chosen: 0, x: shown.left, y: shown.bottom, range })
  }

  const choose = (name: string) => {
    if (menu) put(name, menu.range)
    setMenu(null)
  }

  const keys = (event: KeyboardEvent) => {
    // `selectionchange` comes late: a key right after End would still find the caret in a name.
    remember()
    if (menu) {
      const move = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0
      if (move) {
        event.preventDefault()
        setMenu({ ...menu, chosen: (menu.chosen + move + menu.options.length) % menu.options.length })
        return
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault()
        choose(menu.options[menu.chosen])
        return
      }
      if (event.key === "Escape") {
        event.preventDefault()
        event.stopPropagation()
        setMenu(null)
        return
      }
    }
    if ((event.metaKey || event.ctrlKey) && (event.key === "Enter" || event.key.toLowerCase() === "s")) {
      event.preventDefault()
      onSave()
    }
  }

  const paste = (event: ClipboardEvent) => {
    event.preventDefault()
    const selection = document.getSelection()
    if (!selection?.rangeCount) return
    const range = selection.getRangeAt(0)
    range.deleteContents()
    const text = document.createTextNode(event.clipboardData.getData("text/plain"))
    range.insertNode(text)
    selection.collapse(text, text.length)
    emit()
  }

  return (
    <>
      <div
        ref={box}
        className="mention-text nodrag nopan nowheel"
        role="textbox"
        aria-multiline="true"
        aria-label={label}
        data-drop-target=""
        data-field={place}
        data-placeholder={label}
        contentEditable="plaintext-only"
        suppressContentEditableWarning
        spellCheck={false}
        onFocus={() => store.editing.set(asset)}
        onInput={() => {
          emit()
          look()
        }}
        onKeyDown={keys}
        onPaste={paste}
        onBlur={() => {
          setMenu(null)
          onSave()
        }}
      />
      {menu &&
        createPortal(
          <ul className="name-menu" style={{ left: menu.x, top: menu.y }} role="listbox">
            {menu.options.map((name, i) => (
              <li key={name} role="option" aria-selected={i === menu.chosen} className={i === menu.chosen ? "on" : ""} onMouseDown={(event) => { event.preventDefault(); choose(name) }}>
                {name}
              </li>
            ))}
          </ul>,
          document.body,
        )}
      <MentionThumbs text={value} besides={asset} />
    </>
  )
}

function MentionThumb({ name }: { name: string }) {
  const store = useCanvasStore()
  const view = useElement(store.nameOf(name) ?? name)
  const shown = view && shownOf(view)
  if (!shown || shown.absent || shown.field === "audio") return null
  return (
    <span className="mention-thumb" title={name}>
      <Thumb sha256={shown.sha256} cardWidth={120} />
    </span>
  )
}

/** The pictures a text mentions, small, under it. */
function MentionThumbs({ text, besides }: { text: string; besides: string }) {
  const mentioned = mentionsIn(text).filter((name) => name !== besides)
  if (!mentioned.length) return null
  return (
    <div className="mention-thumbs">
      {mentioned.map((name) => (
        <MentionThumb key={name} name={name} />
      ))}
    </div>
  )
}
