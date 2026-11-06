import { DrawerFrame } from "./Drawer"

export function Stub({ title }: { title: string }) {
  return (
    <DrawerFrame title={title}>
      <p className="muted">Not built yet.</p>
    </DrawerFrame>
  )
}
