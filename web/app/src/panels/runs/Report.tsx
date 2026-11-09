import type { ReactNode } from "react"
import type { Moved, Report as Measurements } from "../../api/calcifer"
import { bytes, ms } from "../format"

function Table({ head, rows }: { head: string[]; rows: ReactNode[][] }) {
  if (!rows.length) return null
  return (
    <div className="scrollx">
      <table className="grid">
        <thead>
          <tr>
            {head.map((name) => (
              <th key={name}>{name}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, at) => (
            <tr key={at}>
              {row.map((cell, column) => (
                <td key={column}>{cell}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

const moved = (m?: Moved) => (m?.bytes ? `${bytes(m.bytes)}${m.gbps ? ` at ${m.gbps.toFixed(1)} Gb/s` : ""}` : "")

/** Where its time went, the models it ran, its kernels and each GPU's memory: worked out by Calcifer, only formatted here. */
export function Report({ report }: { report: Measurements }) {
  return (
    <>
      {Boolean(report.attention_pins?.length) && <p className="muted">Attention kernel pinned: {report.attention_pins?.join(", ")}</p>}
      {report.measured?.map((measured) => (
        <div className="measured" key={measured.by}>
          {(report.measured?.length ?? 0) > 1 && <h4>{measured.by === "run" ? "The run" : `Call ${measured.by}`}</h4>}
          <Table
            head={["Stage", "Time", ""]}
            rows={(measured.stages ?? []).map((stage) => [
              stage.name,
              ms(stage.ms),
              stage.steps ? `${stage.steps.count} steps, ${ms(stage.steps.each_ms)} each` : stage.legs?.map((leg) => `${leg.name} ${ms(leg.ms)}`).join(", "),
            ])}
          />
          <Table
            head={["Slot", "Model", "Lane"]}
            rows={(measured.models ?? []).map((model) => [
              model.slot,
              `${model.repository}${model.release ? `@${model.release}` : ""}${model.adapters?.length ? ` + ${model.adapters.map((adapter) => adapter.model).join(", ")}` : ""}`,
              model.lane || model.precision,
            ])}
          />
          <Table
            head={["Component", "Attention", "GEMM"]}
            rows={(measured.components ?? []).map((component) => [
              component.component,
              component.attention?.join(", "),
              component.gemm?.map((gemm) => `${gemm.kernel} ${gemm.format}${gemm.fallback ? ` (wanted ${gemm.fallback.wanted}: ${gemm.fallback.reason})` : ""}`).join(", "),
            ])}
          />
          <Table
            head={["GPU", "Kernel", "State", "Compile", ""]}
            rows={(measured.kernels ?? []).map((kernel) => [
              kernel.gpu === undefined || kernel.gpu === null ? "" : `GPU ${kernel.gpu}`,
              kernel.kernel,
              kernel.served ? `${kernel.state}, served` : kernel.state,
              kernel.compile_ms ? ms(kernel.compile_ms) : "",
              kernel.detail,
            ])}
          />
          <Table
            head={["GPU", "Kept", "Leased", "Store", "Stall", "Peak"]}
            rows={(measured.memory ?? []).map((memory) => [
              `GPU ${memory.gpu}`,
              moved(memory.kept),
              moved(memory.leased),
              moved(memory.store),
              memory.stall_ms ? ms(memory.stall_ms) : "",
              memory.peak ? bytes(memory.peak) : "",
            ])}
          />
        </div>
      ))}
    </>
  )
}
