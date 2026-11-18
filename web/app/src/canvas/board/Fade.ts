import { glide } from "./Camera"

/** Focus's fade (Paul, 11-17): every box but the one in Focus fades out as the camera moves into it, and back in as it moves out, on the
 *  move's own timeline: its duration and curve, begun with it and advanced by the frames that advance it. Turned on the way, each box goes
 *  on from the opacity it has. By the node's id, which a rename keeps. */
export class Fade {
  private hiding = false
  private keep: string | null = null
  private elapsed = 0
  private duration = 0
  /** Each box's opacity when this fade began: `rest`, but for those in `from`. */
  private rest = 1
  private from = new Map<string, number>()
  /** Asks the canvas for a frame. */
  wake: () => void = () => {}

  /** Box `id`'s opacity now, 0 to 1. */
  of(id: string): number {
    return this.at(this.from.get(id) ?? this.rest, !this.hiding || id === this.keep ? 1 : 0)
  }

  /** Still fading: another frame. */
  get moving() { return this.elapsed < this.duration }

  /** A frame's time, as the camera's move is given it (Stage). */
  advance(ms: number) { if (this.moving) this.elapsed += ms }

  /** Every box but `keep` (none: null) goes, over `ms`. */
  hide(keep: string | null, ms: number) { this.to(true, keep, ms) }
  /** Every box comes back, over `ms`. */
  show(ms: number) { this.to(false, this.keep, ms) }

  private at(start: number, end: number) {
    const t = this.duration > 0 ? this.elapsed / this.duration : 1
    return t >= 1 ? end : start + (end - start) * glide(t)
  }

  private to(hiding: boolean, keep: string | null, ms: number) {
    const rest = this.at(this.rest, this.hiding ? 0 : 1)
    const from = new Map<string, number>()
    for (const id of new Set([...this.from.keys(), this.keep, keep])) {
      if (id === null) continue
      const at = this.of(id)
      if (at !== rest) from.set(id, at)
    }
    this.hiding = hiding
    this.keep = keep
    this.elapsed = 0
    this.duration = Math.max(0, ms)
    this.rest = rest
    this.from = from
    this.wake()
  }
}
