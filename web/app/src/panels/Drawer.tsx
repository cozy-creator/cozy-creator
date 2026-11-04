import type { ReactNode } from "react"
import { Link } from "react-router-dom"
import { useTo } from "../app/Shell"
import { Icon } from "../ui/Icon"

/** A drawer's frame: a title, and closing it, which leaves the canvas as it was. `back` is the list a detail came from. */
export function DrawerFrame({ title, children, back, actions }: { title: string; children: ReactNode; back?: string; actions?: ReactNode }) {
  const to = useTo()
  return (
    <>
      <div className="drawer-head">
        {back && (
          <Link className="iconb" to={back} aria-label="Back">
            <Icon name="back" />
          </Link>
        )}
        <b>{title}</b>
        <span className="sp" />
        {actions}
        <Link className="iconb" to={to()} aria-label="Close">
          <Icon name="close" />
        </Link>
      </div>
      <div className="drawer-body">{children}</div>
    </>
  )
}
