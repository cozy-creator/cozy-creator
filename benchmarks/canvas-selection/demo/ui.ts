import type { DemoActions, DemoMode, DemoUI, DemoUIState } from './ui-types';

const paths: Record<string, string> = {
  undo: '<path d="M8 5 3 10l5 5M3 10h10a6 6 0 0 1 0 12" transform="translate(0 -3)"/>',
  redo: '<path d="m16 5 5 5-5 5m5-5H11a6 6 0 0 0 0 12" transform="translate(0 -3)"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  minus: '<path d="M5 12h14"/>',
  image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8" cy="9" r="1.5"/><path d="m4 18 6-6 4 4 3-3 4 4"/>',
  video: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="m10 8 6 4-6 4z"/>',
  upload: '<path d="M12 16V3m-4 4 4-4 4 4M4 15v5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-5"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M15 8V4H4v11h4"/>',
  trash: '<path d="M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7"/>',
  fit: '<path d="M4 9V4h5m6 0h5v5M4 15v5h5m6 0h5v-5"/>',
  home: '<path d="m3 11 9-8 9 8M6 9v12h12V9m-8 12v-7h4v7"/>',
  tour: '<path d="m8 5 12 7-12 7z"/>',
  cube: '<path d="m12 2 9 5v10l-9 5-9-5V7zm-9 5 9 5 9-5m-9 5v10"/>',
  reset: '<path d="M3 10a9 9 0 1 1 2 8M3 4v6h6"/>',
  help: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 1 1 4.1 1.9c-1.4.9-1.6 1.1-1.6 2.6M12 17h.01"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
};
const icon = (name: string) => `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${paths[name] || ''}</svg>`;

/** Creator's frame around one live scene; its controls never sit inside the canvas surface. */
export function createDemoUI(actions: DemoActions): DemoUI {
  const root = document.createElement('div');
  root.className = 'demo-ui';
  root.innerHTML = `
    <header class="demo-topbar" aria-label="Project toolbar">
      <button class="demo-brand" data-action="home" title="Home" aria-label="Home"><span aria-hidden="true">c<span>.</span></span></button>
      <div class="demo-history">
        <button class="demo-icon-button" data-action="undo" title="Undo (Ctrl/⌘ Z)" aria-label="Undo" disabled>${icon('undo')}</button>
        <button class="demo-icon-button" data-action="redo" title="Redo (Ctrl/⌘ Shift Z)" aria-label="Redo" disabled>${icon('redo')}</button>
      </div>
      <button class="demo-icon-button" data-action="addImage" title="Add an image" aria-label="Add image">${icon('plus')}</button>
      <div class="demo-project"><strong>landscape_studies</strong><span>project</span></div>
    </header>

    <div class="demo-top-actions" aria-label="Local scene actions">
      <span class="demo-local"><i aria-hidden="true"></i>local</span>
      <button class="demo-render-button" data-action="addImage" title="Add a local sample image">Render sample</button>
      <button class="demo-icon-button" data-action="help" title="Canvas help" aria-label="Canvas help" aria-controls="demo-help" aria-expanded="false">${icon('help')}</button>
    </div>

    <aside class="demo-tools" aria-label="Scene tools">
      <section class="demo-tool-section">
        <h2>Nodes</h2>
        <button class="demo-tool-button" data-action="addImage">${icon('image')}<span>Image</span><span class="demo-row-plus" aria-hidden="true">+</span></button>
        <button class="demo-tool-button" data-action="addVideo">${icon('video')}<span>Video</span><span class="demo-row-plus" aria-hidden="true">+</span></button>
        <button class="demo-tool-button demo-upload" data-action="files">${icon('upload')}<span>Upload files</span></button>
      </section>
      <section class="demo-tool-section demo-model-list">
        <h2>Tools</h2>
        <div class="demo-model-row"><span class="demo-model-mark">Q</span><div><span>Qwen Image 2.1</span><small>Image</small></div></div>
        <div class="demo-model-row"><span class="demo-model-mark">F</span><div><span>Flux Krea</span><small>Image</small></div></div>
        <div class="demo-model-row"><span class="demo-model-mark demo-video-mark">M</span><div><span>MiniMax H3</span><small>Video</small></div></div>
      </section>
      <section class="demo-tool-section demo-scene-section">
        <label class="demo-field-label" for="demo-scene-count">Scene</label>
        <select id="demo-scene-count" aria-label="Scene size" title="Replace the scene with this many sample nodes">
          <option value="100">100 nodes</option>
          <option value="1000" selected>1,000 nodes</option>
          <option value="5000">5,000 nodes</option>
        </select>
        <div class="demo-selection-actions">
          <button data-action="duplicate" disabled>${icon('copy')}<span>Duplicate</span></button>
          <button data-action="remove" disabled>${icon('trash')}<span>Delete</span></button>
        </div>
        <button class="demo-reset" data-action="reset">${icon('reset')}<span>Reset scene</span></button>
      </section>
      <div class="demo-sidebar-note">Double-click a card to open it.</div>
    </aside>

    <div class="demo-experiments" aria-label="Preview controls">
      <button class="demo-small-button" data-action="tour" title="Take a camera tour of the scene">${icon('tour')}<span>Tour</span></button>
      <button class="demo-small-button" data-action="model" aria-pressed="false">${icon('cube')}<span data-role="model-label">3D model</span></button>
      <label class="demo-check"><input data-option="tilt" type="checkbox"><span>Tilt</span></label>
      <label class="demo-check"><input data-option="stats" type="checkbox"><span>Stats</span></label>
      <div class="demo-live-stats" hidden aria-live="off"><span><b data-role="fps">—</b> FPS</span><span>p95 <b data-role="p95">—</b> ms</span></div>
    </div>

    <section class="demo-comparison" aria-label="Renderer comparison">
      <div class="demo-comparison-caption"><span class="demo-prototype-label">LOCAL PROTOTYPE</span><span data-role="scene-summary">Loading scene…</span><span class="demo-ready" data-role="ready">Starting…</span></div>
      <div class="demo-mode-tabs" role="tablist" aria-label="Canvas renderer">
        <button role="tab" data-mode="react-dom" aria-selected="false"><kbd>1</kbd><span><b>A</b> React DOM</span></button>
        <button role="tab" data-mode="pixi" aria-selected="false"><kbd>2</kbd><span><b>B</b> PixiJS <small>WebGL</small></span></button>
        <button role="tab" data-mode="reactflow" aria-selected="false"><kbd>3</kbd><span><small>Reference</small> React Flow</span></button>
      </div>
    </section>

    <nav class="demo-navigation" aria-label="Canvas view">
      <button class="demo-small-button" data-action="home" title="Return to the starting view">Home</button>
      <button class="demo-small-button" data-action="fit" title="Fit all nodes in view">Fit</button>
      <div class="demo-zoom">
        <button data-action="zoomOut" aria-label="Zoom out" title="Zoom out">${icon('minus')}</button>
        <output data-role="zoom" aria-label="Canvas zoom">—</output>
        <button data-action="zoomIn" aria-label="Zoom in" title="Zoom in">${icon('plus')}</button>
      </div>
    </nav>

    <aside id="demo-help" class="demo-help" aria-label="Canvas help" hidden>
      <div class="demo-help-heading"><h2>Make yourself at home.</h2><button class="demo-icon-button" data-action="closeHelp" aria-label="Close help">${icon('close')}</button></div>
      <p>Same photos, layout and camera. Switch the renderer and keep working.</p>
      <dl class="demo-shortcuts">
        <div><dt>Pan</dt><dd>Drag empty space, <kbd>Space</kbd> + drag, or scroll</dd></div>
        <div><dt>Zoom</dt><dd><kbd>Ctrl</kbd> / <kbd>⌘</kbd> + scroll</dd></div>
        <div><dt>Move a card</dt><dd>Drag its image or heading</dd></div>
        <div><dt>Select more</dt><dd><kbd>Shift</kbd> + click or drag a box</dd></div>
        <div><dt>Open a card</dt><dd>Double-click</dd></div>
        <div><dt>Delete</dt><dd><kbd>Delete</kbd> / <kbd>Backspace</kbd></dd></div>
        <div><dt>Undo</dt><dd><kbd>Ctrl</kbd> / <kbd>⌘</kbd> + <kbd>Z</kbd></dd></div>
        <div><dt>Switch renderer</dt><dd><kbd>1</kbd> React DOM · <kbd>2</kbd> PixiJS · <kbd>3</kbd> React Flow</dd></div>
      </dl>
      <p class="demo-help-footnote">A local comparison, with sample media. Changes stay in this tab.</p>
    </aside>
    <div class="demo-toast" role="status" aria-live="polite" hidden></div>
  `;
  document.body.appendChild(root);

  const required = <T extends Element>(selector: string) => root.querySelector<T>(selector)!;
  const help = required<HTMLElement>('.demo-help');
  const helpButton = required<HTMLButtonElement>('[data-action="help"]');
  const toast = required<HTMLElement>('.demo-toast');
  const count = required<HTMLSelectElement>('#demo-scene-count');
  const tilt = required<HTMLInputElement>('[data-option="tilt"]');
  const stats = required<HTMLInputElement>('[data-option="stats"]');
  const liveStats = required<HTMLElement>('.demo-live-stats');
  const modelButton = required<HTMLButtonElement>('[data-action="model"]');
  const undoButton = required<HTMLButtonElement>('[data-action="undo"]');
  const redoButton = required<HTMLButtonElement>('[data-action="redo"]');
  const duplicateButton = required<HTMLButtonElement>('[data-action="duplicate"]');
  const removeButton = required<HTMLButtonElement>('[data-action="remove"]');
  const ready = required<HTMLElement>('[data-role="ready"]');
  const sceneSummary = required<HTMLElement>('[data-role="scene-summary"]');
  const zoom = required<HTMLOutputElement>('[data-role="zoom"]');
  const fps = required<HTMLElement>('[data-role="fps"]');
  const p95 = required<HTMLElement>('[data-role="p95"]');
  const modelLabel = required<HTMLElement>('[data-role="model-label"]');
  const modeButtons = [...root.querySelectorAll<HTMLButtonElement>('[data-mode]')];
  const listeners: Array<() => void> = [];
  let current: DemoUIState | undefined;
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;

  const showHelp = (open: boolean) => {
    help.hidden = !open;
    helpButton.setAttribute('aria-expanded', String(open));
    if (open) actions.help();
  };
  const callbacks: Record<string, () => void> = {
    home: () => actions.home(), fit: () => actions.fit(), undo: () => actions.undo(), redo: () => actions.redo(),
    addImage: () => actions.addImage(), addVideo: () => actions.addVideo(), files: () => actions.files(),
    duplicate: () => actions.duplicate(), remove: () => actions.remove(), tour: () => actions.tour(), reset: () => actions.reset(),
    zoomOut: () => actions.zoom(-1), zoomIn: () => actions.zoom(1),
    model: () => actions.model(!(current?.model ?? false)), help: () => showHelp(Boolean(help.hidden)), closeHelp: () => showHelp(false),
  };
  const listen = (element: EventTarget, type: string, callback: EventListener) => {
    element.addEventListener(type, callback);
    listeners.push(() => element.removeEventListener(type, callback));
  };
  for (const button of root.querySelectorAll<HTMLButtonElement>('[data-action]')) {
    listen(button, 'click', () => { if (!button.disabled) callbacks[button.dataset.action!](); });
  }
  for (const button of modeButtons) listen(button, 'click', () => actions.mode(button.dataset.mode as DemoMode));
  listen(count, 'change', () => actions.count(Number(count.value)));
  listen(tilt, 'change', () => actions.tilt(tilt.checked));
  listen(stats, 'change', () => actions.stats(stats.checked));
  listen(document, 'keydown', event => {
    if ((event as KeyboardEvent).key === 'Escape' && !help.hidden) {
      event.preventDefault();
      event.stopPropagation();
      showHelp(false);
      helpButton.focus();
    }
  });
  const text = (element: Element, value: string) => { if (element.textContent !== value) element.textContent = value; };
  const disabled = (button: HTMLButtonElement, value: boolean) => { if (button.disabled !== value) button.disabled = value; };
  const numbers = new Intl.NumberFormat('en-US');

  return {
    update(state) {
      if (destroyed) return;
      current = state;
      for (const button of modeButtons) {
        const active = button.dataset.mode === state.mode;
        button.classList.toggle('is-active', active);
        if (button.getAttribute('aria-selected') !== String(active)) button.setAttribute('aria-selected', String(active));
        if (active && state.backend && button.title !== state.backend) button.title = state.backend;
      }
      if ([100, 1000, 5000].includes(state.total) && count.value !== String(state.total)) count.value = String(state.total);
      disabled(undoButton, !state.canUndo);
      disabled(redoButton, !state.canRedo);
      disabled(duplicateButton, state.selected === 0);
      disabled(removeButton, state.selected === 0);
      text(sceneSummary, `${numbers.format(state.total)} nodes · ${numbers.format(state.visible)} visible${state.selected ? ` · ${numbers.format(state.selected)} selected` : ''}`);
      text(zoom, `${Math.round(state.zoom * 100)}%`);
      const fraction = Math.max(0, Math.min(1, state.ready > 1 ? state.ready / 100 : state.ready));
      text(ready, state.busy ? 'Switching…' : fraction >= 1 ? 'Ready' : `Loading ${Math.floor(fraction * 100)}%`);
      ready.classList.toggle('is-busy', state.busy);
      ready.classList.toggle('is-ready', !state.busy && fraction >= 1);
      if (tilt.checked !== state.tilt) tilt.checked = state.tilt;
      if (stats.checked !== state.stats) stats.checked = state.stats;
      if (liveStats.hidden === state.stats) liveStats.hidden = !state.stats;
      if (state.stats) {
        text(fps, Number.isFinite(state.fps) ? String(Math.round(state.fps!)) : '—');
        text(p95, Number.isFinite(state.p95) ? state.p95!.toFixed(1) : '—');
      }
      if (modelButton.getAttribute('aria-pressed') !== String(state.model)) modelButton.setAttribute('aria-pressed', String(state.model));
      text(modelLabel, state.model ? 'Close 3D' : '3D model');
    },
    toast(message) {
      if (destroyed) return;
      clearTimeout(toastTimer);
      text(toast, message);
      toast.hidden = false;
      toastTimer = setTimeout(() => { toast.hidden = true; }, 4000);
    },
    destroy() {
      if (destroyed) return;
      destroyed = true;
      clearTimeout(toastTimer);
      for (const remove of listeners) remove();
      root.remove();
    },
  };
}
