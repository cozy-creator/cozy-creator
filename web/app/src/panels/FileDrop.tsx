import { type ReactNode, useEffect, useRef, useState } from "react"
import { moveBytes } from "../api/upload"
import { useConnection } from "../app/Connection"
import { bytes } from "./format"

/** A file on its way to Calcifer, or there: `digest` is what a request names it by, and is never shown. */
export interface Dropped {
  id: number
  name: string
  size?: number
  moving?: { stage: string; done: number }
  attempt?: number
  digest?: string
  failed?: string
}

let nextId = 0

/** Files by what's dropped or chosen, sent by the one upload path as they arrive, whole or resuming. */
export function useFiles(start: Dropped[] = [], projectId?: string) {
  const { client } = useConnection()
  const [files, setFiles] = useState<Dropped[]>(start)
  const stops = useRef(new Map<number, AbortController>())

  useEffect(() => {
    const held = stops.current
    return () => {
      for (const stop of held.values()) stop.abort()
    }
  }, [])

  const patch = (id: number, change: Partial<Dropped>) => setFiles((now) => now.map((file) => (file.id === id ? { ...file, ...change } : file)))

  const add = (chosen: File[], onSent?: (file: Dropped) => void) => {
    const added = chosen.map((file) => ({ file, row: { id: ++nextId, name: file.name, size: file.size } as Dropped }))
    setFiles((now) => [...now, ...added.map(({ row }) => row)])
    for (const { file, row } of added) {
      const stop = new AbortController()
      stops.current.set(row.id, stop)
      moveBytes(
        client,
        file,
        stop.signal,
        (moving) => patch(row.id, { moving }),
        (attempt) => patch(row.id, { attempt: attempt || undefined }),
        projectId,
      )
        .then((digest) => {
          patch(row.id, { digest, moving: undefined, attempt: undefined })
          onSent?.({ ...row, digest })
        })
        .catch((error: unknown) => {
          if (!stop.signal.aborted) patch(row.id, { failed: error instanceof Error ? error.message : String(error), moving: undefined })
        })
        .finally(() => stops.current.delete(row.id))
    }
    return added.map(({ row }) => row.id)
  }

  const remove = (id: number) => {
    stops.current.get(id)?.abort()
    setFiles((now) => now.filter((file) => file.id !== id))
  }

  return { files, add, remove, moving: files.some((file) => !file.digest && !file.failed) }
}

/** Somewhere to drop files, or click for a selector. */
export function DropZone({ accept, multiple, onFiles, children }: { accept?: string; multiple?: boolean; onFiles: (files: File[]) => void; children: ReactNode }) {
  const picker = useRef<HTMLInputElement>(null)
  const [over, setOver] = useState(false)
  return (
    <div
      className={`dropzone ${over ? "over" : ""}`}
      onDragOver={(event) => {
        event.preventDefault()
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(event) => {
        event.preventDefault()
        event.stopPropagation()
        setOver(false)
        const dropped = [...event.dataTransfer.files]
        if (dropped.length) onFiles(multiple ? dropped : dropped.slice(0, 1))
      }}
    >
      <button type="button" className="more" onClick={() => picker.current?.click()}>
        {children}
      </button>
      <input
        ref={picker}
        className="picker"
        type="file"
        accept={accept}
        multiple={multiple}
        onChange={(event) => {
          const chosen = [...(event.target.files ?? [])]
          event.target.value = ""
          if (chosen.length) onFiles(chosen)
        }}
      />
    </div>
  )
}

export function FileRow({ file, label, onRemove, children }: { file: Dropped; label?: string; onRemove: () => void; children?: ReactNode }) {
  const state = file.failed
    ? file.failed
    : file.moving
      ? `${file.moving.stage === "reading" ? "Reading" : "Sending"} ${Math.round(file.moving.done * 100)}%${file.attempt ? ` · attempt ${file.attempt}/5` : ""}`
      : file.digest
        ? ""
        : "Waiting"
  return (
    <div className={`filerow ${file.failed ? "bad" : ""}`}>
      {label && <code>{label}</code>}
      <span className="nm">{file.name}</span>
      {file.size !== undefined && <small>{bytes(file.size)}</small>}
      <small>{state}</small>
      {children}
      <span className="sp" />
      <button type="button" className="iconb" aria-label={`Take out ${file.name}`} onClick={onRemove}>
        ×
      </button>
    </div>
  )
}
