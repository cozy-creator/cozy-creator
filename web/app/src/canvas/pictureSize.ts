/** A picture's size as a browser draws it, read from its file's first bytes: PNG, JPEG (turned by its EXIF orientation), GIF, WebP and BMP.
 *  Null for anything else, or a header it can't read. */
export async function pictureSize(file: File): Promise<{ width: number; height: number } | null> {
  const bytes = new DataView(await file.slice(0, HEAD_BYTES).arrayBuffer())
  const size = png(bytes) ?? gif(bytes) ?? webp(bytes) ?? bmp(bytes) ?? jpeg(bytes)
  return size && size.width > 0 && size.height > 0 ? size : null
}

/** Enough for a camera JPEG's EXIF (at most 64 KiB) and what comes before its frame header. */
const HEAD_BYTES = 256 * 1024

type Size = { width: number; height: number }
const ascii = (bytes: DataView, at: number, length: number) =>
  at + length <= bytes.byteLength ? String.fromCharCode(...Array.from({ length }, (_, i) => bytes.getUint8(at + i))) : ""

function png(bytes: DataView): Size | null {
  if (bytes.byteLength < 24 || bytes.getUint32(0) !== 0x89504e47 || ascii(bytes, 12, 4) !== "IHDR") return null
  return { width: bytes.getUint32(16), height: bytes.getUint32(20) }
}

function gif(bytes: DataView): Size | null {
  if (bytes.byteLength < 10 || !ascii(bytes, 0, 6).match(/^GIF8[79]a$/)) return null
  return { width: bytes.getUint16(6, true), height: bytes.getUint16(8, true) }
}

function webp(bytes: DataView): Size | null {
  if (bytes.byteLength < 30 || ascii(bytes, 0, 4) !== "RIFF" || ascii(bytes, 8, 4) !== "WEBP") return null
  const chunk = ascii(bytes, 12, 4)
  if (chunk === "VP8 ") return { width: bytes.getUint16(26, true) & 0x3fff, height: bytes.getUint16(28, true) & 0x3fff }
  if (chunk === "VP8L") {
    const bits = bytes.getUint32(21, true)
    return { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 }
  }
  if (chunk === "VP8X") {
    const three = (at: number) => bytes.getUint8(at) | (bytes.getUint8(at + 1) << 8) | (bytes.getUint8(at + 2) << 16)
    return { width: three(24) + 1, height: three(27) + 1 }
  }
  return null
}

function bmp(bytes: DataView): Size | null {
  if (bytes.byteLength < 26 || ascii(bytes, 0, 2) !== "BM") return null
  if (bytes.getUint32(14, true) === 12) return { width: bytes.getUint16(18, true), height: bytes.getUint16(20, true) }
  return { width: Math.abs(bytes.getInt32(18, true)), height: Math.abs(bytes.getInt32(22, true)) }
}

/** Its frame header's size, across if its EXIF orientation turns it a quarter (5 to 8). */
function jpeg(bytes: DataView): Size | null {
  if (bytes.byteLength < 4 || bytes.getUint16(0) !== 0xffd8) return null
  let turned = false
  let at = 2
  while (at + 4 <= bytes.byteLength) {
    if (bytes.getUint8(at) !== 0xff) return null
    const marker = bytes.getUint8(at + 1)
    // Fill bytes, and markers with no length.
    if (marker === 0xff) {
      at += 1
      continue
    }
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) {
      at += 2
      continue
    }
    const length = bytes.getUint16(at + 2)
    if (marker === 0xe1 && ascii(bytes, at + 4, 6) === "Exif\0\0") turned = (orientation(bytes, at + 10, length - 8) ?? 1) >= 5
    // Any start of frame but DHT (C4), JPG (C8) and DAC (CC).
    if (marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc) {
      if (at + 9 > bytes.byteLength) return null
      const height = bytes.getUint16(at + 5)
      const width = bytes.getUint16(at + 7)
      return turned ? { width: height, height: width } : { width, height }
    }
    if (marker === 0xda) return null
    at += 2 + length
  }
  return null
}

/** EXIF's IFD0 Orientation (tag 0x0112), in the TIFF block at `start`. */
function orientation(bytes: DataView, start: number, length: number): number | null {
  const end = Math.min(bytes.byteLength, start + length)
  if (start + 8 > end) return null
  const little = ascii(bytes, start, 2) === "II"
  if (!little && ascii(bytes, start, 2) !== "MM") return null
  const ifd = start + bytes.getUint32(start + 4, little)
  if (ifd + 2 > end) return null
  const entries = bytes.getUint16(ifd, little)
  for (let i = 0; i < entries; i++) {
    const entry = ifd + 2 + i * 12
    if (entry + 12 > end) return null
    if (bytes.getUint16(entry, little) === 0x0112) return bytes.getUint16(entry + 8, little)
  }
  return null
}
