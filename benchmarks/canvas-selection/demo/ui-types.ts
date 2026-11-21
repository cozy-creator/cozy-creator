export type DemoMode = 'react-dom' | 'pixi' | 'reactflow';
export interface DemoUIState {
 mode: DemoMode; total: number; visible: number; selected: number; zoom: number; ready: number;
 canUndo: boolean; canRedo: boolean; busy: boolean; stats: boolean; tilt: boolean; model: boolean;
 fps?: number; p95?: number; backend?: string;
}
export interface DemoActions {
 mode(mode: DemoMode): void; count(count: number): void; home(): void; fit(): void; zoom(direction: number): void;
 undo(): void; redo(): void; addImage(): void; addVideo(): void; files(): void; duplicate(): void; remove(): void;
 tour(): void; tilt(enabled: boolean): void; model(enabled: boolean): void; stats(enabled: boolean): void;
 help(): void; reset(): void;
}
export interface DemoUI { update(state: DemoUIState): void; toast(message: string): void; destroy(): void; }
