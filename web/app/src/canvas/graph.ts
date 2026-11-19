import type { Entity, Project } from "../types"

// What a project's names anchor to, from the state alone: where what reading it said is shown.

function subsOf(entities: Entity[]): Map<string, string> {
  const owner = new Map<string, string>()
  for (const entity of entities) for (const sub of entity.subs ?? []) owner.set(sub.name, entity.name)
  return owner
}

/** Where a name sits: an element, or a sub-entity's row in one. A link of an entity, `<kyogre.image>`, sits where its entity does. */
function anchors(state: Project) {
  const elements = new Set([...state.assets.map((asset) => asset.name), ...(state.entities ?? []).map((entity) => entity.name)])
  const owners = subsOf(state.entities ?? [])
  return (name: string): { element: string; sub?: string } | null => {
    for (let at = name; ; at = at.slice(0, at.lastIndexOf("."))) {
      if (elements.has(at)) return { element: at }
      const owner = owners.get(at)
      if (owner) return { element: owner, sub: at }
      if (!at.includes(".")) return null
    }
  }
}

/** What reading the project said, by the card that shows it: a sub-entity's or a link's on its entity. What's of no card is the project's. */
export function warningsOf(state: Project): { byElement: Map<string, string[]>; ofProject: string[] } {
  const anchor = anchors(state)
  const byElement = new Map<string, string[]>()
  const ofProject: string[] = []
  for (const { name, message } of state.warnings ?? []) {
    const element = name ? anchor(name)?.element : undefined
    if (element) byElement.set(element, [...(byElement.get(element) ?? []), message])
    else ofProject.push(message)
  }
  return { byElement, ofProject }
}

export interface Box {
  x: number
  y: number
  w: number
  h: number
}
