/** A matched media-card scene. x/y are world coordinates; camera x/y are screen translations. */
export interface Card { id: string; x: number; y: number; width: number; height: number; asset: number; label: string; status: string }
export interface Camera { x: number; y: number; zoom: number }
export interface BenchOptions {
  cards: Card[];
  camera: Camera;
  width: number;
  height: number;
  /** Absolute URL for the identical thumbnail resource used by every backend. Tier is a source width. */
  assetUrl(asset: number, tier: number): string;
  /** Shared requested pixel tier, using actual visible image width and DPR. Changes only at tier thresholds. */
  tier(camera: Camera): number;
  /** True for world rect intersection with viewport plus 64 screen-pixel overscan. */
  visible(card: Card, camera: Camera): boolean;
  /** LOD2: image+simple border only; LOD1: image+name; LOD0: full name,status,version,selection chrome. */
  lod(camera: Camera): 0 | 1 | 2;
  /** Called on actual render (not merely RAF). */
  onDraw(): void;
}
export interface Adapter {
  /** init resolves when renderer constructed, not necessarily all thumbnails loaded. */
  setCamera(camera: Camera): void;
  setSelected(id: string | null): void;
  moveCard(id: string, x: number, y: number): void;
  /** Must report backend truthfully and source/image/texture counters where available. */
  stats(): Record<string, unknown>;
  /** Current loaded-and-displayed images divided by currently visible cards. */
  readyFraction(): number;
  destroy(): void;
}
export type CreateAdapter = (host: HTMLElement, options: BenchOptions) => Promise<Adapter>;
