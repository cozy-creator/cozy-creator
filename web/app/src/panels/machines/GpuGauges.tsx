import { type PointerEvent, useState } from "react"
import type { Gpu, GpuReading } from "../../api/calcifer"
import { type GpuTrace, TRACE_MS } from "../../api/gpus"
import { useNow } from "../common"
import "./GpuGauges.css"
import { bytes } from "../format"

/** A reading not followed by another this long is no longer live: the machine stopped saying (unreachable, ended). */
const STALE_MS = 5_000

/** What holds a GPU back now (cl-1191), in plain words, each once. */
export function limitsOf(reading: GpuReading): { word: string; why: string }[] {
  const said = new Map<string, string>()
  for (const name of reading.throttles ?? []) {
    if (name === "power_cap") said.set("Power-limited", "it draws all the power its limit allows, so its clocks are held down")
    else if (name === "sw_thermal" || name === "hw_thermal") said.set("Thermal", "it's slowing itself to cool down")
    else if (name === "hw_slowdown") said.set("Hardware slowdown", "its board is slowing it, for heat or power")
    else if (name === "power_brake") said.set("Power brake", "an outside power brake is slowing it")
  }
  return [...said].map(([word, why]) => ({ word, why }))
}

/** `used` of `total` in `total`'s unit: "6.1/8GB". */
function share(used: number, total: number, between = "/"): string {
  const unit = bytes(total).replace(/^[\d.]+/, "")
  const scale = 1024 ** ["B", "KB", "MB", "GB", "TB", "PB"].indexOf(unit)
  const value = used / scale
  return `${value < 10 ? value.toFixed(1).replace(/\.0$/, "") : value.toFixed(0)}${between}${bytes(total)}`
}

/** A machine's newest reading, while it's live. */
export function useLive(trace: GpuTrace) {
  const last = trace.at(-1)
  const now = useNow(Boolean(last), 1000)
  return last && now - last.heard < STALE_MS ? last : undefined
}

/** The last minute of one GPU's busy share, a line over its area, newest at the right; hovered, the reading under the pointer. */
function Trace({ trace, index, wide = false }: { trace: GpuTrace; index: number; wide?: boolean }) {
  // Where the pointer is across it, 0 to 1: the reading under it is read again as new ones come.
  const [hover, setHover] = useState<number | null>(null)
  const points = trace.flatMap(({ at, gpus }) => gpus.filter((gpu) => gpu.index === index).map((gpu) => ({ at, busy: gpu.busy_percent })))
  const last = points.at(-1)
  if (!last) return null
  const [w, h] = [120, 24]
  const x = (at: number) => w - ((last.at - at) / TRACE_MS) * w
  const y = (busy: number) => h - 1 - (busy / 100) * (h - 2)
  const line = points.map((point) => `${x(point.at).toFixed(1)},${y(point.busy).toFixed(1)}`).join(" ")
  const area = `${x(points[0].at).toFixed(1)},${h} ${line} ${x(last.at).toFixed(1)},${h}`
  const low = Math.min(...points.map((point) => point.busy))
  const high = Math.max(...points.map((point) => point.busy))
  const under = (fraction: number) => {
    const at = last.at - (1 - fraction) * TRACE_MS
    return points.reduce((nearest, point) => (Math.abs(point.at - at) < Math.abs(nearest.at - at) ? point : nearest))
  }
  const shown = hover === null ? null : under(hover)
  const pick = (event: PointerEvent<SVGSVGElement>) => {
    const box = event.currentTarget.getBoundingClientRect()
    setHover(Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)))
  }
  return (
    <span className={`trace ${wide ? "wide" : ""}`}>
      <svg
        viewBox={`0 0 ${w} ${h}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={`GPU ${index} busy over the last minute: ${low}% to ${high}%`}
        onPointerMove={pick}
        onPointerLeave={() => setHover(null)}
      >
        <line className="trace-base" x1={0} x2={w} y1={h - 0.5} y2={h - 0.5} vectorEffect="non-scaling-stroke" />
        <polygon className="trace-area" points={area} />
        <polyline className="trace-line" points={line} vectorEffect="non-scaling-stroke" />
        {shown && <line className="trace-at" x1={x(shown.at)} x2={x(shown.at)} y1={0} y2={h} vectorEffect="non-scaling-stroke" />}
      </svg>
      {shown && (
        <span className="trace-tip" style={{ left: `${(x(shown.at) / w) * 100}%` }}>
          <b>{shown.busy}%</b> {Math.round((last.at - shown.at) / 1000)}s ago
        </span>
      )}
    </span>
  )
}

function Meter({ kind, label, share: fill }: { kind: string; label: string; share: number }) {
  const percent = Math.max(0, Math.min(100, fill * 100))
  return (
    <span className={`meter ${kind}`} role="meter" aria-label={label} aria-valuenow={Math.round(percent)} aria-valuemin={0} aria-valuemax={100}>
      <i style={{ width: `${percent}%` }} />
    </span>
  )
}

/** A machine's GPUs on its row of the machines list (cl-1191, Paul 11-17: "like a throttle, from 0 -> 100%, like what runpod has"): each
 *  one's busy share as a bar, the figure and its last minute beside it, and under the bar its memory, power against its limit,
 *  temperature and what holds it back now. */
export function GpuStrip({ trace, gpus }: { trace: GpuTrace; gpus: Gpu[] }) {
  const live = useLive(trace)
  if (!live?.gpus.length) return null
  const many = live.gpus.length > 1
  return (
    <div className={`gpu-strip ${many ? "many" : ""}`}>
      {live.gpus.map((reading) => {
        const total = reading.memory_total_bytes || gpus.find((gpu) => gpu.index === reading.index)?.memory_bytes || 0
        const limits = limitsOf(reading)
        return (
          <div className="gauge" key={reading.index} data-gpu={reading.index}>
            {many && <span className="gauge-gpu">GPU {reading.index}</span>}
            <Meter kind="busy" label={`GPU ${reading.index} busy`} share={reading.busy_percent / 100} />
            <b className="gauge-busy">{reading.busy_percent}%</b>
            <Trace trace={trace} index={reading.index} />
            <span className="gauge-facts">
              <span className="gauge-fact" title="Memory in use">
                {total ? share(reading.memory_used_bytes, total) : bytes(reading.memory_used_bytes)}
              </span>
              {reading.power_w != null && (
                <span className="gauge-fact" title={reading.power_limit_w ? "Power draw of its limit" : "Power draw"}>
                  {`${reading.power_w}${reading.power_limit_w ? `/${reading.power_limit_w}` : ""}W`}
                </span>
              )}
              {reading.temperature_c != null && (
                <span className="gauge-fact" title="Temperature">
                  {`${reading.temperature_c}°C`}
                </span>
              )}
              {limits.map((limit) => (
                <span className="gauge-limit" key={limit.word} title={`${limit.word}: ${limit.why}`}>
                  {limit.word}
                </span>
              ))}
            </span>
          </div>
        )
      })}
    </div>
  )
}

/** One GPU in its machine's drawer: busy share and its last minute, memory, power against its limit, temperature and clock, each a row,
 *  and what holds it back now, said. */
export function GpuGauges({ trace, reading, total }: { trace: GpuTrace; reading: GpuReading; total: number }) {
  const memory = reading.memory_total_bytes || total
  const limits = limitsOf(reading)
  const clock = reading.sm_mhz != null && `clock ${reading.sm_mhz.toLocaleString()}${reading.sm_mhz_max ? ` of ${reading.sm_mhz_max.toLocaleString()}` : ""} MHz`
  const heat = [reading.temperature_c != null && `${reading.temperature_c}°C`, clock].filter(Boolean).join(" · ")
  return (
    <>
      <div className="gpu-use">
        <span>Busy</span>
        <Meter kind="busy" label={`GPU ${reading.index} busy`} share={reading.busy_percent / 100} />
        <b>{reading.busy_percent}%</b>
        <Trace trace={trace} index={reading.index} wide />
        <span>Memory</span>
        <Meter kind="memory" label={`GPU ${reading.index} memory in use`} share={memory ? reading.memory_used_bytes / memory : 0} />
        <b>{memory ? `${bytes(reading.memory_used_bytes)} of ${bytes(memory)}` : bytes(reading.memory_used_bytes)}</b>
        {reading.power_w != null && (
          <>
            <span>Power</span>
            {reading.power_limit_w ? <Meter kind="power" label={`GPU ${reading.index} power of its limit`} share={reading.power_w / reading.power_limit_w} /> : <span />}
            <b>
              {reading.power_w}
              {reading.power_limit_w ? ` of ${reading.power_limit_w}` : ""}W
            </b>
          </>
        )}
        {heat && (
          <>
            <span>Temp</span>
            <span className="gpu-heat">{heat}</span>
          </>
        )}
      </div>
      {limits.map((limit) => (
        <p className="gpu-why hot gpu-limit" key={limit.word}>
          <b>{limit.word} now:</b> {limit.why}.
        </p>
      ))}
    </>
  )
}
