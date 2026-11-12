import { type ClipboardEvent, type KeyboardEvent, type MouseEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import type { Media } from "../types"
import type { CanvasStore } from "./CanvasStore"
import { LET_GO, type LetGo } from "./carry"
import { ClipRange } from "./ClipRange"
import { useCanvasStore, useElement } from "./context"
import { WANTED } from "./controls"
import { Thumb } from "./Media"
import { angleOf, baseOf, degrees, FPS, framesIn, rangeOf, ranged, retext, timecode } from "./ranges"
import { chipWords, mentionsIn, withMentions } from "./text"
import { isSelf, mentionFrom, NO_BARE_SELF } from "./versions"

/** What a chip says: its name in its brackets, a range of a video as timecode, and an orbit's angle in degrees. */
function chipSaid(store: CanvasStore, name: string, asset: string): string {
  const angled = angleOf(name)
  if (angled) return `${chipWords(angled.base)} ${degrees(angled)}`
  const range = rangeOf(name)
  if (!range) return chipWords(name)
  const file = store.fileOf(name, asset)
  const fps = file?.fps ?? FPS
  return `${chipWords(range.base)} ${timecode(range.from, fps)}–${timecode(range.to ?? framesIn(file?.duration_ms, fps), fps)}`
}

/** Why a mention in this text isn't sent, or null: words take only words, and a prompt only what it takes. A text box is always put in as its words. */
function unsent(store: CanvasStore, name: string, asset: string, takes: Media[] | undefined): string | null {
  const kind = store.kindOf(name, asset)
  if (!takes || !kind || kind === "text" || takes.includes(kind as Media)) return null
  if (!takes.length) return `${baseOf(name)} goes as its name: this takes words only`
  return `${baseOf(name)} isn't sent: this takes ${takes.map((held) => WANTED[held]).join(" or ")} only`
}

/** A name as a chip: one gone is struck through, and one this text doesn't send, `refused`, is dimmed and says why. */
function chip(name: string, words: string, missing: boolean, refused: string | null): HTMLElement {
  const el = document.createElement("span")
  el.className = missing ? "chip missing" : refused ? "chip refused" : "chip"
  el.contentEditable = "false"
  el.dataset.mention = name
  el.textContent = words
  el.title = missing ? (isSelf(name) ? NO_BARE_SELF : `${baseOf(name)} is no longer here`) : (refused ?? name)
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
  placeholder,
  missing,
  takes,
  focus,
  thumbs = true,
  onDraft,
  onSave,
  onSubmit,
}: {
  asset: string
  place: string
  value: string
  label: string
  /** What it reads when it's empty, its label if nothing else. */
  placeholder?: string
  missing: string[]
  /** What its mentions may send: none for words, any when it doesn't say. What else it mentions stays its name, marked. */
  takes?: Media[]
  focus?: boolean
  /** The pictures it mentions, small, under it: none where they're behind it. A video's range is under it either way. */
  thumbs?: boolean
  onDraft: (text: string) => void
  onSave: () => void
  onSubmit?: () => void
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
      const said = part.mention ? store.mentionOf(inner, asset) : null
      if (part.mention && (missing.includes(inner) || said?.gone || (isSelf(inner) && !said))) return chip(inner, chipSaid(store, inner, asset), true, null)
      if (said) return chip(inner, chipSaid(store, inner, asset), false, unsent(store, inner, asset, takes))
      return document.createTextNode(part.text)
    })
    const merged = (held: ChildNode[]) => held.reduce((said, node) => said + (node instanceof HTMLElement ? `\u0000${node.className}:${node.dataset.mention}:${node.textContent}:${node.title}\u0000` : node.textContent), "")
    const trimmed = (said: string) => said.replace(/\s+$/, "")
    if (trimmed(merged([...el.childNodes])) === trimmed(merged(nodes))) return
    el.replaceChildren(...nodes)
    // What it pointed into is gone: a name dropped now goes at the end.
    caret.current = null
  }, [value, missing, store, names, asset, takes])

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
      const said = store.mentionOf(name, asset)
      const made = chip(name, chipSaid(store, name, asset), !said || Boolean(said.gone), said ? unsent(store, name, asset, takes) : null)
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
    [emit, store, asset, takes],
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

  // A take carried out of a box and let go in this text: that version, where it was let go.
  useEffect(() => {
    const el = box.current
    if (!el) return
    const heard = (event: Event) => {
      const { load, x, y } = (event as LetGo).detail
      if (!load.take?.from || !load.take.number) return
      event.stopPropagation()
      put(mentionFrom(asset, load.take.from, load.take.number), document.caretRangeFromPoint?.(x, y) ?? caret.current)
      onSave()
    }
    el.addEventListener(LET_GO, heard)
    return () => el.removeEventListener(LET_GO, heard)
  }, [asset, put, onSave])

  useEffect(() => {
    if (!focus || !box.current) return
    box.current.focus({ preventScroll: true })
    // Typing goes on from what it says.
    window.getSelection()?.selectAllChildren(box.current)
    window.getSelection()?.collapseToEnd()
  }, [focus])

  useEffect(() => {
    document.addEventListener("selectionchange", remember)
    return () => document.removeEventListener("selectionchange", remember)
  }, [remember])

  /** A click on a name puts the caret just after it, at once: what a browser does with a click there differs, and a late `selectionchange` would let a key go nowhere. */
  const clickName = (event: MouseEvent) => {
    const name = (event.target as Element).closest("[data-mention]")
    const parent = name?.parentNode
    if (!name || !parent || !box.current?.contains(name)) return
    const selection = document.getSelection()
    selection?.collapse(parent, [...parent.childNodes].indexOf(name) + 1)
    if (selection?.rangeCount) caret.current = selection.getRangeAt(0).cloneRange()
  }

  /** After each keystroke: `<name>` finished by hand is a chip, and a `<` being typed offers names. */
  const look = () => {
    const selection = document.getSelection()
    const at = selection?.anchorNode
    if (!selection?.isCollapsed || !(at instanceof Text) || !box.current?.contains(at)) return setMenu(null)
    const before = at.data.slice(0, selection.anchorOffset)
    const closed = before.match(/<([^<>\s]+)>$/)
    if (closed && isSelf(closed[1]) && !store.nameOf(closed[1])) say(NO_BARE_SELF)
    if (closed && (store.mentionOf(closed[1], asset) || isSelf(closed[1]))) {
      const range = document.createRange()
      range.setStart(at, selection.anchorOffset - closed[0].length)
      range.setEnd(at, selection.anchorOffset)
      setMenu(null)
      return put(closed[1], range)
    }
    const open = before.match(/<([^<>\s]*)$/)
    const options = open ? store.namesLike(open[1], asset).filter((name) => !unsent(store, name, asset, takes)).slice(0, 8) : []
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
    if (event.nativeEvent.isComposing) return
    if (event.ctrlKey && event.key.toLowerCase() === "j") {
      event.preventDefault()
      event.stopPropagation()
      const selection = document.getSelection()
      if (selection?.rangeCount) {
        const range = selection.getRangeAt(0)
        range.deleteContents()
        const line = document.createTextNode("\n")
        range.insertNode(line)
        selection.collapse(line, 1)
        emit()
      }
      setMenu(null)
      return
    }
    if (event.key === "Enter" && event.shiftKey) {
      event.stopPropagation()
      setMenu(null)
      return
    }
    if (onSubmit && event.key === "Enter" && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault()
      event.stopPropagation()
      if (menu) choose(menu.options[menu.chosen])
      onSubmit()
      return
    }
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

  // What's copied is what the file holds, so a name keeps its brackets.
  const copy = (event: ClipboardEvent) => {
    const selection = document.getSelection()
    if (!selection?.rangeCount || selection.isCollapsed) return
    const held = document.createElement("div")
    held.append(selection.getRangeAt(0).cloneContents())
    event.clipboardData.setData("text/plain", read(held))
    event.preventDefault()
  }

  const cut = (event: ClipboardEvent) => {
    copy(event)
    if (!event.defaultPrevented) return
    document.getSelection()?.getRangeAt(0).deleteContents()
    emit()
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
        data-placeholder={placeholder ?? label}
        contentEditable="plaintext-only"
        suppressContentEditableWarning
        spellCheck={false}
        onInput={() => {
          emit()
          look()
        }}
        onKeyDown={keys}
        onClick={clickName}
        onCopy={copy}
        onCut={cut}
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
      <MentionThumbs
        text={value}
        besides={asset}
        takes={takes}
        pictures={thumbs}
        onRange={(old, now) => {
          onDraft(retext(box.current ? read(box.current) : value, old, now))
          onSave()
        }}
      />
    </>
  )
}

function MentionThumb({ name, asset, takes, pictures, onRange }: { name: string; asset: string; takes?: Media[]; pictures: boolean; onRange(old: string, now: string): void }) {
  const store = useCanvasStore()
  const said = store.mentionOf(name, asset)
  useElement(said?.name ?? baseOf(name))
  const shown = store.fileOf(name, asset)
  if (!shown || shown.absent || shown.field === "audio" || unsent(store, name, asset, takes)) return null
  // An orbit's angle has no in and out to drag: its chip says it.
  if (angleOf(name)) return null
  if (shown.field === "video") {
    const fps = shown.fps ?? FPS
    const frames = framesIn(shown.duration_ms, fps)
    const range = rangeOf(name)
    return (
      <span className="mention-clip" title={name}>
        <ClipRange sha256={shown.sha256} frames={frames} fps={fps} from={range?.from ?? 0} to={range?.to ?? frames} width={240} label={baseOf(name)} onSet={(from, to) => onRange(name, ranged(baseOf(name), from, to, frames))} />
      </span>
    )
  }
  if (!pictures) return null
  return (
    <span className="mention-thumb" title={name}>
      <Thumb sha256={shown.sha256} cardWidth={120} />
    </span>
  )
}

/** The pictures a text mentions, small, under it, and each video's frames with its in and out. */
function MentionThumbs({ text, besides, takes, pictures, onRange }: { text: string; besides: string; takes?: Media[]; pictures: boolean; onRange(old: string, now: string): void }) {
  const mentioned = mentionsIn(text).filter((name) => name !== besides)
  if (!mentioned.length) return null
  return (
    <div className="mention-thumbs">
      {/* By place and what it names: a new range is the same strip, and keeps its handle's focus. */}
      {mentioned.map((name, i) => (
        <MentionThumb key={`${i}:${baseOf(name)}`} name={name} asset={besides} takes={takes} pictures={pictures} onRange={onRange} />
      ))}
    </div>
  )
}
