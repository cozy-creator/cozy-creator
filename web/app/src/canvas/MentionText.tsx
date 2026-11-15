import { type ClipboardEvent, type KeyboardEvent, type MouseEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"
import type { Unused } from "../api/calcifer"
import { say } from "../ui/notes"
import { useSignal } from "../ui/signal"
import type { CanvasStore } from "./CanvasStore"
import { LET_GO, type LetGo } from "./carry"
import { ClipRange } from "./ClipRange"
import { useCanvasStore, useElement } from "./context"
import { Thumb } from "./Media"
import { angleOf, baseOf, degrees, FPS, framesIn, rangeOf, ranged, retext, timecode } from "./ranges"
import { chipWords, mentionsIn, withMentions } from "./text"
import { paintTriggers, takesWords, typedIn, unpaint, useTriggers } from "./triggers"
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

/** Why its tool can't use `name` in this text, as Calcifer found it (cl-944), or null. */
const leftOut = (unused: Unused[], name: string): string | null => unused.find((one) => one.mention === name)?.why ?? null

/** A name as a chip: one gone is struck through, saying whether it was deleted, and so is one its tool can't use here, `unused`, saying why. */
function chip(name: string, words: string, missing: boolean, unused: string | null, deleted = false): HTMLElement {
  const el = document.createElement("span")
  el.className = missing ? "chip missing" : unused ? "chip unused" : "chip"
  el.contentEditable = "false"
  el.dataset.mention = name
  el.textContent = words
  const gone = isSelf(name) ? NO_BARE_SELF : deleted ? `${baseOf(name)} was deleted` : `${baseOf(name)} is no longer here`
  el.title = missing ? gone : (unused ?? name)
  return el
}

/** A mention's chip, as loading, typing and pasting all make it: one gone, or one its tool can't use here, struck through; none for a name this
 *  project doesn't know, which stays words. */
function chipFor(store: CanvasStore, name: string, asset: string, missing: string[], unused: Unused[]): HTMLElement | null {
  const said = store.mentionOf(name, asset)
  // A deleted box's name is struck at once, as Calcifer will say it (cl-919).
  const deleted = !said && store.deletedName(name)
  if (missing.includes(name) || said?.gone || deleted || (isSelf(name) && !said)) return chip(name, chipSaid(store, name, asset), true, null, deleted)
  return said ? chip(name, chipSaid(store, name, asset), false, leftOut(unused, name)) : null
}

/** Text as the box shows it: each `<name>` its chip, the rest words. */
function nodesOf(store: CanvasStore, text: string, asset: string, missing: string[], unused: Unused[]): (HTMLElement | Text)[] {
  return withMentions(text).map((part) => (part.mention && chipFor(store, part.text.slice(1, -1), asset, missing, unused)) || document.createTextNode(part.text))
}

/** Where the caret is in what `box` says, counted as the file holds it, or null when it isn't in it. */
function caretIn(box: HTMLElement): number | null {
  const selection = document.getSelection()
  if (!selection?.rangeCount || !selection.anchorNode || !box.contains(selection.anchorNode)) return null
  const before = document.createRange()
  before.selectNodeContents(box)
  before.setEnd(selection.anchorNode, selection.anchorOffset)
  const held = document.createElement("div")
  held.append(before.cloneContents())
  return read(held).length
}

/** The caret `offset` into what `box` says: in words where it falls in them, else just after the chip it falls in. */
function caretAt(box: HTMLElement, offset: number) {
  const selection = document.getSelection()
  let left = offset
  for (const [at, child] of [...box.childNodes].entries()) {
    const length = child instanceof HTMLElement && child.dataset.mention ? child.dataset.mention.length + 2 : (child.textContent ?? "").length
    if (left <= length) return child instanceof Text ? selection?.collapse(child, left) : selection?.collapse(box, at + 1)
    left -= length
  }
  selection?.collapse(box, box.childNodes.length)
}

/** A place in the words: a node, and an offset in it. */
interface Place {
  node: globalThis.Node
  offset: number
}

/** The caret's place under the point, as the browser finds it. */
function caretFromPoint(x: number, y: number): Place | null {
  const found = document as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: globalThis.Node; offset: number } | null
    caretRangeFromPoint?: (x: number, y: number) => Range | null
  }
  const position = found.caretPositionFromPoint?.(x, y)
  if (position) return { node: position.offsetNode, offset: position.offset }
  const range = found.caretRangeFromPoint?.(x, y)
  return range ? { node: range.startContainer, offset: range.startOffset } : null
}

/** Where in `box`'s words the point is: there, else, off them, their nearer end; on a name, just after it. */
function placeIn(box: HTMLElement, x: number, y: number): Place {
  const rect = box.getBoundingClientRect()
  const [start, end] = [{ node: box, offset: 0 }, { node: box, offset: box.childNodes.length }]
  if (y > rect.bottom || (y >= rect.top && x > rect.right)) return end
  if (y < rect.top || x < rect.left) return start
  const at = caretFromPoint(x, y)
  if (!at || !box.contains(at.node)) return end
  const name = (at.node instanceof Element ? at.node : at.node.parentElement)?.closest("[data-mention]")
  if (name?.parentNode && box.contains(name)) return { node: name.parentNode, offset: [...name.parentNode.childNodes].indexOf(name) + 1 }
  return at
}

/** Where `at`, in `from`, falls in `to`: moved by what changed before it, and to the end of what changed if it was in it. */
function carried(from: string, to: string, at: number): number {
  let prefix = 0
  while (prefix < from.length && prefix < to.length && from[prefix] === to[prefix]) prefix++
  let suffix = 0
  while (suffix < from.length - prefix && suffix < to.length - prefix && from[from.length - 1 - suffix] === to[to.length - 1 - suffix]) suffix++
  if (at <= prefix) return at
  if (at >= from.length - suffix) return at + to.length - from.length
  return to.length - suffix
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
  // What its tool can't use here, as Calcifer said it: struck through, never sent.
  const told = useElement(asset)?.asset?.unused
  const unused = useMemo(() => (told ?? []).filter((one) => one.at === place), [told, place])
  // Its box's LoRAs' trigger words, drawn where it says them, typed or appended (cl-968).
  const triggers = useTriggers(store, asset)
  const triggerKey = triggers.join("\u0000")
  const paint = useCallback(() => { if (box.current) paintTriggers(box.current, triggerKey ? triggerKey.split("\u0000") : []) }, [triggerKey])

  // While it's being typed in, what it holds is what's typed. Left alone it shows names as they now stand, painted only if that differs,
  // so its caret survives a blur and a drop can still land there.
  // biome-ignore lint/correctness/useExhaustiveDependencies: names tells it a chip may have come or gone
  useLayoutEffect(() => {
    const el = box.current
    if (!el) return
    // What ends a text is its file's to say: what's typed, and the caret in it, stand. Only its marks follow what's now known (a box made, a
    // name Calcifer says is nothing), as they would loaded.
    const typing = document.activeElement === el && read(el).trimEnd() === value.trimEnd()
    const nodes = nodesOf(store, typing ? read(el) : value, asset, missing, unused)
    const mark = (node: Node) => (node instanceof HTMLElement ? `\u0000${node.className}:${node.dataset.mention}:${node.textContent}:${node.title}\u0000` : node.textContent)
    const trimmed = (said: string) => said.replace(/\s+$/, "")
    // Typed, only its chips count: the line breaks a browser keeps as it types are its own.
    const shown = typing ? [...el.querySelectorAll("[data-mention]")] : [...el.childNodes]
    const wanted = typing ? nodes.filter((node) => node instanceof HTMLElement) : nodes
    if (trimmed(shown.map(mark).join("")) === trimmed(wanted.map(mark).join(""))) return
    // Typed in, and what it says moved under the caret (another's words merged in): the caret stays where it was in the words.
    const said = document.activeElement === el ? read(el) : null
    const at = said === null ? null : caretIn(el)
    el.replaceChildren(...nodes)
    if (at !== null && said !== null) return caretAt(el, typing ? at : carried(said, value, at))
    // What it pointed into is gone: a name dropped now goes at the end.
    caret.current = null
  }, [value, missing, store, names, asset, unused])

  // biome-ignore lint/correctness/useExhaustiveDependencies: what it says, painted again whenever it's shown anew
  useLayoutEffect(() => paint(), [paint, value, names, missing])
  useEffect(() => {
    const el = box.current
    return () => { if (el) unpaint(el) }
  }, [])

  const emit = useCallback(() => {
    if (box.current) onDraft(read(box.current))
  }, [onDraft])

  /** Words where `at` says, else at the end, spaced from what's beside them: a trigger clicked on its LoRA's row. */
  const insertWords = useCallback(
    (words: string, at: Range | null) => {
      const el = box.current
      if (!el) return
      const inside = at && el.contains(at.startContainer)
      const before = inside ? (at.startContainer.textContent ?? "").slice(0, at.startOffset) : read(el)
      const after = inside ? (at.endContainer.textContent ?? "").slice(at.endOffset) : ""
      const node = document.createTextNode(`${before && !/\s$/.test(before) ? " " : ""}${words}${after && !/^\s/.test(after) ? " " : ""}`)
      if (inside) {
        at.deleteContents()
        at.insertNode(node)
      } else el.append(node)
      if (document.activeElement === el) document.getSelection()?.collapse(node, node.length)
      emit()
      paint()
      onSave()
    },
    [emit, paint, onSave],
  )
  useEffect(() => takesWords(asset, place, (words) => insertWords(words, caret.current)), [asset, place, insertWords])

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
      const made = chipFor(store, name, asset, missing, unused) ?? chip(name, chipSaid(store, name, asset), true, null)
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
    [emit, store, asset, missing, unused],
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

  /**
   * A press in the words, dragged, selects from where it was pressed to where the pointer is, out past the words too, as in any text. The
   * page does this, not the browser: with a name in its words, Chrome put the caret at the pointer on every move of a drag and selected
   * nothing, with no script or change of the words between (cl-973). A double or triple click, Shift, a press on a name or on words already
   * selected (to drag them) stay the browser's.
   */
  const pressWords = (event: MouseEvent) => {
    const el = box.current
    if (!el || event.button !== 0 || event.detail > 1 || event.shiftKey || event.altKey || event.metaKey || event.ctrlKey) return
    if ((event.target as Element).closest("[data-mention]")) return
    const selection = document.getSelection()
    const from = placeIn(el, event.clientX, event.clientY)
    if (selection && !selection.isCollapsed && selection.rangeCount && selection.getRangeAt(0).isPointInRange(from.node, from.offset)) return
    event.preventDefault()
    el.focus({ preventScroll: true })
    selection?.collapse(from.node, from.offset)
    const move = (moved: globalThis.MouseEvent) => {
      if (!(moved.buttons & 1)) return end()
      // Past the top or the bottom of words that scroll, they scroll on, as a selection dragged there does.
      const rect = el.getBoundingClientRect()
      if (moved.clientY > rect.bottom) el.scrollTop += moved.clientY - rect.bottom
      else if (moved.clientY < rect.top) el.scrollTop -= rect.top - moved.clientY
      const to = placeIn(el, moved.clientX, moved.clientY)
      document.getSelection()?.setBaseAndExtent(from.node, from.offset, to.node, to.offset)
    }
    const end = () => {
      window.removeEventListener("mousemove", move, true)
      window.removeEventListener("mouseup", end, true)
    }
    window.addEventListener("mousemove", move, true)
    window.addEventListener("mouseup", end, true)
  }

  /** After each keystroke: `<name>` finished by hand is a chip, and a `<` being typed offers names. */
  const look = () => {
    const selection = document.getSelection()
    const at = selection?.anchorNode
    if (!selection?.isCollapsed || !(at instanceof Text) || !box.current?.contains(at)) return setMenu(null)
    const before = at.data.slice(0, selection.anchorOffset)
    const closed = before.match(/<([^<>\s]+)>$/)
    if (closed && isSelf(closed[1]) && !store.nameOf(closed[1])) say(NO_BARE_SELF)
    if (closed && chipFor(store, closed[1], asset, missing, unused)) {
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
      // With names offered, Enter takes the one chosen and nothing else, as in Slack's or an editor's list; with none, it renders.
      if (menu) return choose(menu.options[menu.chosen])
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
      // Cmd/Ctrl+Enter in any prompt renders (Paul, 11-12).
      if (event.key === "Enter" && onSubmit) {
        event.stopPropagation()
        onSubmit()
      }
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

  // What's pasted is marked as it would be loaded: its mentions are chips at once, however many lines, whatever program it came from.
  const paste = (event: ClipboardEvent) => {
    event.preventDefault()
    const selection = document.getSelection()
    if (!selection?.rangeCount) return
    const range = selection.getRangeAt(0)
    range.deleteContents()
    const end = document.createTextNode("")
    const pasted = document.createDocumentFragment()
    pasted.append(...nodesOf(store, event.clipboardData.getData("text/plain"), asset, missing, unused), end)
    range.insertNode(pasted)
    selection.collapse(end, 0)
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
          paint()
        }}
        onFocus={() => typedIn(asset, place)}
        onKeyDown={keys}
        onMouseDown={pressWords}
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
        unused={unused}
        pictures={thumbs}
        onRange={(old, now) => {
          onDraft(retext(box.current ? read(box.current) : value, old, now))
          onSave()
        }}
      />
    </>
  )
}

function MentionThumb({ name, asset, unused, pictures, onRange }: { name: string; asset: string; unused: Unused[]; pictures: boolean; onRange(old: string, now: string): void }) {
  const store = useCanvasStore()
  const said = store.mentionOf(name, asset)
  useElement(said?.name ?? baseOf(name))
  const shown = store.fileOf(name, asset)
  if (!shown || shown.absent || shown.field === "audio" || leftOut(unused, name)) return null
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
function MentionThumbs({ text, besides, unused, pictures, onRange }: { text: string; besides: string; unused: Unused[]; pictures: boolean; onRange(old: string, now: string): void }) {
  const mentioned = mentionsIn(text).filter((name) => name !== besides)
  if (!mentioned.length) return null
  return (
    <div className="mention-thumbs">
      {/* By place and what it names: a new range is the same strip, and keeps its handle's focus. */}
      {mentioned.map((name, i) => (
        <MentionThumb key={`${i}:${baseOf(name)}`} name={name} asset={besides} unused={unused} pictures={pictures} onRange={onRange} />
      ))}
    </div>
  )
}
