/** Drawn, not typed: a font's `✎` is a different picture on every computer, or none. All 20 by 20, in the text's colour. */
const STROKES = {
  menu: "M3 6h14M3 10h14M3 14h14",
  close: "M5.5 5.5l9 9M14.5 5.5l-9 9",
  plus: "M10 4.5v11M4.5 10h11",
  minus: "M4.5 10h11",
  back: "M12 4.5L6.5 10l5.5 5.5",
  forward: "M8 4.5l5.5 5.5L8 15.5",
  edit: "M12.8 4.2l3 3L7.2 15.8l-3.7.7.7-3.7z",
  picture: "M3.5 4.5h13v11h-13zM3.5 13l3.8-3.6 2.8 2.6 2.3-1.9 4.1 3.6",
  video: "M3.5 5h13v10h-13zM6.5 5v10M13.5 5v10M3.5 8.3h3M3.5 11.7h3M13.5 8.3h3M13.5 11.7h3",
  sound: "M4 8.5v3M7.3 6v8M10.6 3.5v13M13.9 6v8M17 8.5v3",
  touch: "M10 3.5l1.5 4.9 5 1.6-5 1.6L10 16.5l-1.5-4.9-5-1.6 5-1.6z",
  download: "M10 3v10M5.5 8.8L10 13.3l4.5-4.5M4 16.5h12",
  package: "M10 2.8l6.5 3.4v7.6L10 17.2l-6.5-3.4V6.2zM3.5 6.2L10 9.6l6.5-3.4M10 9.6v7.6M6.8 4.5l6.5 3.4",
  undo: "M7.5 4.5L4 8l3.5 3.5M4 8h7.5a4.5 4.5 0 010 9H8",
  redo: "M12.5 4.5L16 8l-3.5 3.5M16 8H8.5a4.5 4.5 0 000 9H12",
  history: "M10 3.5a6.5 6.5 0 110 13 6.5 6.5 0 010-13zM10 6.5V10l2.5 1.6",
  orbit: "M16.2 8.6A6.5 6.5 0 104 12.6M16.5 3.8v4.8h-4.8",
} as const

const FILLS = {
  play: "M6.5 4.3v11.4c0 .5.6.8 1 .5l8.8-5.7c.4-.3.4-.8 0-1L7.5 3.8c-.4-.3-1 0-1 .5z",
} as const

export type IconName = keyof typeof STROKES | keyof typeof FILLS

export function Icon({ name }: { name: IconName }) {
  const stroke = name in STROKES ? STROKES[name as keyof typeof STROKES] : null
  return (
    <svg className="icon" viewBox="0 0 20 20" width="14" height="14" aria-hidden="true" fill={stroke ? "none" : "currentColor"} stroke={stroke ? "currentColor" : "none"} strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d={stroke ?? FILLS[name as keyof typeof FILLS]} />
    </svg>
  )
}
