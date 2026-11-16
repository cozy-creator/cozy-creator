// A player full screen (cl-1080; Paul, 11-16: "like youtube-style"): the Fullscreen API on the player's own element, WebKit's prefixed one
// where that's all there is. Its frame (FocusFrame) is what goes full screen; what offers it is a control in the frame (a transport, an
// orbit's bar), so a frame with nothing to play has none.
import { createContext, useContext, useEffect } from "react"
import { signal } from "../ui/signal"

type Prefixed = Document & { webkitFullscreenElement?: Element | null; webkitFullscreenEnabled?: boolean; webkitExitFullscreen?: () => void }
const prefixed = document as Prefixed

/** What the page shows full screen now. */
export const fullNow = (): Element | null => document.fullscreenElement ?? prefixed.webkitFullscreenElement ?? null
/** Whether this browser can show an element full screen (an iPhone's Safari can't). */
export const canFull = () => Boolean(document.fullscreenEnabled ?? prefixed.webkitFullscreenEnabled)

/** The same, as it changes. */
export const fullElement = signal<Element | null>(fullNow())
let leftAt = -Infinity
const heard = () => {
  const now = fullNow()
  if (fullElement.get() && !now) leftAt = performance.now()
  fullElement.set(now)
}
document.addEventListener("fullscreenchange", heard)
document.addEventListener("webkitfullscreenchange", heard)

export function enterFull(element: HTMLElement) {
  if (element.requestFullscreen) void element.requestFullscreen({ navigationUI: "hide" }).catch(() => {})
  else (element as HTMLElement & { webkitRequestFullscreen?: () => void }).webkitRequestFullscreen?.()
}

export function leaveFull() {
  if (!fullNow()) return
  if (document.exitFullscreen) void document.exitFullscreen().catch(() => {})
  else prefixed.webkitExitFullscreen?.()
}

/** Within a moment of leaving full screen: the Esc that left it, if a browser hands it on too, must not also leave Focus. */
export const justLeft = () => performance.now() - leftAt < 400

/** A key typed into words, where F is a letter. */
export const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (target.isContentEditable || target.tagName === "TEXTAREA" || target.tagName === "SELECT" || (target instanceof HTMLInputElement && ["text", "search", "url", "email", "number", "password", "tel"].includes(target.type)))

export interface FullScreen {
  full: boolean
  toggle(): void
  /** A control in the frame offers full screen while it's there; what it returns takes the offer back. */
  offer(): () => void
}
export const FullScreenContext = createContext<FullScreen | null>(null)

/** The frame's full screen, when it can go full screen. `offers`: this control is what offers it (its button, F, a double-click). */
export function useFullScreen(offers = false): FullScreen | null {
  const screen = useContext(FullScreenContext)
  const offer = offers ? screen?.offer : undefined
  useEffect(() => offer?.(), [offer])
  return screen
}
