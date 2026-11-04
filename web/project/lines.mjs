// project.yaml by its lines, the way `approve` edits it: an asset's lines are found by indent, and only they change, so every
// comment, `|` and blank line elsewhere stays byte for byte. Calcifer reads the result as a project before it keeps it.

export const nameKey = name => name.replace(/[_-]/g, " ").trim().split(/\s+/).join(" ").toLowerCase();
const significant = line => line.trim() !== "" && !line.trimStart().startsWith("#");
const indent = line => line.length - line.trimStart().length;
const split = text => text.split(/(?<=\n)/);

// The key a line opens and what follows its colon.
function keyOf(line) {
  const t = line.trim();
  if (t[0] === '"' || t[0] === "'") {
    const end = t.indexOf(t[0], 1);
    const rest = end > 0 ? t.slice(end + 1).trimStart() : "";
    return rest.startsWith(":") ? [t.slice(1, end), rest.slice(1).trim()] : null;
  }
  const colon = t.search(/:(\s|$)/);
  return colon < 0 ? null : [t.slice(0, colon), t.slice(colon + 1).trim()];
}

// Where `assets:` is: the lines of its section and how far its keys are indented.
function section(lines) {
  const open = lines.findIndex(line => /^assets:\s*(#.*)?$/.test(line));
  if (open < 0) return null;
  let end = open + 1;
  while (end < lines.length && !(significant(lines[end]) && indent(lines[end]) === 0)) end++;
  const first = lines.slice(open + 1, end).find(significant);
  return {start: open + 1, end, at: first ? indent(first) : 2};
}

// Asset `name`'s own lines: `text` without the line that names it, unless that line holds its value (`a: {image: x}`), and with
// their shared indent taken off. Lines the next asset's comment holds aren't its. Null when it has no lines of its own.
export function read(source, name) {
  const lines = split(source), s = section(lines);
  if (!s) return null;
  let key = s.start;
  while (key < s.end && !(significant(lines[key]) && indent(lines[key]) === s.at && nameKey(keyOf(lines[key])?.[0] ?? "") === nameKey(name))) key++;
  if (key === s.end) return null;
  let to = key + 1;
  while (to < s.end && !(significant(lines[to]) && indent(lines[to]) <= s.at)) to++;
  while (to > key + 1 && !significant(lines[to - 1])) to--;
  const value = keyOf(lines[key])[1];
  const inline = value !== "" && !value.startsWith("#");
  const from = inline ? key : key + 1;
  const kept = lines.slice(from, to);
  const margin = inline ? s.at : Math.min(s.at + 2, ...kept.filter(line => line.trim() !== "").map(indent));
  return {text: kept.map(line => line.trim() === "" ? line : line.slice(margin)).join(""), from, to, margin};
}

// `source` with asset `name`'s lines now `text`, which `read` gave and a person changed.
export function write(source, name, text) {
  const place = read(source, name);
  if (!place) throw new Error(`${name} has no lines of its own under assets:`);
  const lines = split(source);
  const body = split(text.endsWith("\n") || text === "" ? text : text + "\n").map(line => line.trim() === "" ? line : " ".repeat(place.margin) + line);
  if (place.from > 0 && !lines[place.from - 1].endsWith("\n")) lines[place.from - 1] += "\n";
  return [...lines.slice(0, place.from), ...body, ...lines.slice(place.to)].join("");
}

// `source` with a new asset at the end of `assets:`.
export function add(source, name, kind) {
  let lines = split(source), s = section(lines);
  if (!s) {
    lines = split(source + (source.endsWith("\n") || source === "" ? "" : "\n") + "\nassets:\n");
    s = section(lines);
  }
  let after = s.end;
  while (after > s.start && !significant(lines[after - 1])) after--;
  if (after > 0 && !lines[after - 1].endsWith("\n")) lines[after - 1] += "\n";
  const pad = " ".repeat(s.at), written = /^[\w .-]+$/.test(name) ? name : JSON.stringify(name);
  const block = `${after > s.start ? "\n" : ""}${pad}${written}:\n${pad}  kind: ${kind}\n${pad}  description: |\n${pad}    What it is.\n`;
  return [...lines.slice(0, after), block, ...lines.slice(after)].join("");
}
