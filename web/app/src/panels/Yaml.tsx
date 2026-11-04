import { useQuery } from "@tanstack/react-query"
import { yamlQuery } from "../api/queries"
import { useConnection } from "../app/Connection"
import { useProjectId } from "../app/Shell"
import { say } from "../ui/notes"
import { DrawerFrame } from "./Drawer"
import { useEffect } from "react"

/** The file itself, read-only for now. The one place a hash shows, because the file holds it. */
export function Yaml() {
  const { client } = useConnection()
  const id = useProjectId()!
  const { data, error } = useQuery(yamlQuery(client, id))
  useEffect(() => {
    if (error) say("Couldn't read project.yaml", error)
  }, [error])
  return (
    <DrawerFrame title="project.yaml">
      <pre className="yaml">{data}</pre>
    </DrawerFrame>
  )
}
