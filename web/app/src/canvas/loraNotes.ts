/** Optional model-card notes; platform catalog metadata takes precedence. */
export interface LoraNote { description: string; source: string; strength?: number }

const NOTES: Record<string, LoraNote> = {}

export const loraNote = (ref: string): LoraNote | undefined => NOTES[ref.split("@")[0]]
