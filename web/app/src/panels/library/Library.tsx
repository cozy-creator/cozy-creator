import type { ReactNode } from "react"
import { Link } from "react-router-dom"
import { useConnection } from "../../app/Connection"
import { useTo } from "../../app/Shell"
import { useSignedIn } from "../common"
import { DrawerFrame } from "../Drawer"

/** The wide drawer's frame: Packages and Models, and what signing in would add here. */
export function LibraryFrame({ on, title, back, children }: { on: "packages" | "models"; title?: string; back?: string; children: ReactNode }) {
  const { client } = useConnection()
  const to = useTo()
  const signedIn = useSignedIn()
  return (
    <DrawerFrame title={title ?? "Library"} back={back}>
      <div className="row tabs">
        <Link className={`tab ${on === "packages" ? "on" : ""}`} to={to("packages")}>
          Packages
        </Link>
        <Link className={`tab ${on === "models" ? "on" : ""}`} to={to("models")}>
          Models
        </Link>
      </div>
      {!signedIn && !client.transport.channel && (
        <p className="muted">
          These are what's public.{" "}
          <Link className="more" to={to("account")}>
            Sign in
          </Link>{" "}
          to see what's private to you as well, and to publish, yank and delete.
        </p>
      )}
      {children}
    </DrawerFrame>
  )
}
