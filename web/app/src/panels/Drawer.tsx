import type { ReactNode } from "react"
import { Link } from "react-router-dom"
import { useTo } from "../app/Shell"

/** A drawer's frame: a title, and closing it, which leaves the canvas as it was. */
export function DrawerFrame({ title, children }: { title: string; children: ReactNode }) {
  const to = useTo()
  return (
    <>
      <div className="drawer-head">
        <b>{title}</b>
        <span className="sp" />
        <Link className="iconb" to={to()} aria-label="Close">
          ×
        </Link>
      </div>
      <div className="drawer-body">{children}</div>
    </>
  )
}
