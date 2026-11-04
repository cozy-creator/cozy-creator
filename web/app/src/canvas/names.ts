// The same rule as `as_name` and `name_key` (`crates/calcifer/src/project/graph.rs`), so a name shows as it will be while it's typed.
// Calcifer picks the name in the end, and the element shows what it got.

/** What can't be in a name becomes `_`. A `.` means a sub-entity or a numbered write, so it can't be typed here. */
export function asName(text: string): string {
  return text
    .split(/[^\p{Alphabetic}\p{N}_-]+/u)
    .filter(Boolean)
    .join("_")
}

/** `asName` for a field being typed in: a separator just typed stays, as `_`, for the next word to follow. */
export function typingName(text: string): string {
  const name = asName(text)
  return name && !name.endsWith("_") && /[^\p{Alphabetic}\p{N}_-]$/u.test(text) ? `${name}_` : name
}

/** `Kyogre Art (1).png` is `kyogre_art_1`. */
export function stemName(fileName: string): string {
  return asName(fileName.replace(/(.)\.[^.]*$/, "$1").toLowerCase())
}

function nameKey(name: string): string {
  return name.replace(/[_-]/g, " ").split(/\s+/).filter(Boolean).join(" ").toLowerCase()
}

/** A taken name gets `_2`. */
export function freeName(name: string, taken: Iterable<string>): string {
  const keys = new Set<string>()
  for (const other of taken) keys.add(nameKey(other))
  if (!keys.has(nameKey(name))) return name
  for (let n = 2; ; n++) {
    const next = `${name}_${n}`
    if (!keys.has(nameKey(next))) return next
  }
}
