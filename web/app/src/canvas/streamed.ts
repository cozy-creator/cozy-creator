import type { Run } from "../api/calcifer"

/** Each run's words as the page last heard them (xs-209). A run's record holds them only while it runs, so what it wrote stays on its box
 *  until its version lands with the same words. The newest few are kept. */
const heard = new Map<string, string>()
const KEPT = 32

/** The words a text tool's run has written so far, while it writes them and until its version lands; none before its first, or once it
 *  failed or was canceled. */
export function streamedWords(run: Run | undefined): string | undefined {
  if (!run || run.status === "failed" || run.status === "canceled") return undefined
  const live = run.text?.find((item) => item.index === 0) ?? run.text?.[0]
  if (!live?.text) return heard.get(run.id)
  heard.delete(run.id)
  heard.set(run.id, live.text)
  for (const id of heard.keys()) {
    if (heard.size <= KEPT) break
    heard.delete(id)
  }
  return live.text
}
