async function containmentProof(input) {
  // Fixed hosted proof actions only. No caller-selected JavaScript or replacement app/client.
  const hash = async (bytes) => [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map((n) => n.toString(16).padStart(2, '0')).join('');
  const card = () => document.querySelector(`.card.here[data-asset="${CSS.escape(input.asset)}"]`) ?? document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(input.asset)}"]`);
  switch (input.kind) {
    case 'ready':
      return { page_ready: typeof globalThis.__cozyFreeze === 'function' && typeof globalThis.__cozyPrepareClose === 'function', cards: document.querySelectorAll('.card[data-asset]').length };
    case 'diagnose': {
      // Closed booleans and counts only: no URL, resource name or page text leaves the page.
      const gate = document.querySelector('main.gate')?.textContent ?? '';
      const resources = performance.getEntriesByType('resource');
      const status = (code) => resources.filter((entry) => entry.responseStatus === code).length;
      const navigation = performance.getEntriesByType('navigation')[0];
      return {
        blank: location.href === 'about:blank', error_page: location.protocol === 'chrome-error:',
        loopback: location.protocol === 'http:' && location.hostname === '127.0.0.1', reloaded: navigation?.type === 'reload',
        navigation_status: navigation?.responseStatus ?? -1, owned_injected: typeof globalThis.__cozyOwned === 'object' && globalThis.__cozyOwned !== null,
        vite_client: [...document.scripts].some((script) => script.src && new URL(script.src).pathname === '/@vite/client'),
        refresh_preamble: typeof globalThis.$RefreshReg$ === 'function', app_mounted: (document.getElementById('root')?.childElementCount ?? 0) > 0,
        gate_connecting: gate.startsWith('Connecting'), gate_signed_out: gate.includes("isn't signed in"), gate_unreachable: gate.includes("isn't answering"),
        gate_opening_project: gate.startsWith('Opening project'), gate_project_failed: gate.startsWith("Couldn't open"), gate_broke: gate.startsWith('That broke'),
        vite_overlay: document.querySelector('vite-error-overlay') !== null, visible: document.visibilityState === 'visible',
        resources: resources.length, resources_failed: resources.filter((entry) => entry.responseStatus >= 400).length, resources_unanswered: status(0),
        resources_403: status(403), resources_404: status(404), resources_503: status(503), resources_504: status(504),
      };
    }
    case 'locate': {
      const box = card(), target = box?.classList.contains('here') ? box : box?.querySelector('.media');
      if (!target) return { found: false };
      const rect = target.getBoundingClientRect(), x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
      // Closed facts for an edit that never opens: what is on top at the press, and which events reach the box.
      const node = box.closest('.react-flow__node'), hit = document.elementFromPoint(x, y);
      if (!globalThis.__cozyProofClicks) {
        const seen = globalThis.__cozyProofClicks = { node: null };
        for (const type of ['mousedown', 'click', 'dblclick']) document.addEventListener(type, (event) => { seen[type] = true; seen[`${type}_in_node`] ||= Boolean(seen.node?.contains(event.target)); }, true);
      }
      Object.assign(globalThis.__cozyProofClicks, { node, mousedown: false, click: false, dblclick: false, mousedown_in_node: false, click_in_node: false, dblclick_in_node: false });
      return { found: rect.width > 0 && rect.height > 0, x, y, hit_target: Boolean(hit && target.contains(hit)), hit_node: Boolean(hit && node?.contains(hit)) };
    }
    case 'focus': {
      const editor = card()?.querySelector(`.mention-text[data-field="${CSS.escape(input.field)}"]`);
      editor?.focus();
      if (editor && document.activeElement === editor) return { found: true };
      // Closed facts only, for a box whose editor does not take focus.
      const frame = await new Promise((done) => { const late = setTimeout(() => done(false), 500); requestAnimationFrame(() => { clearTimeout(late); done(true); }); });
      return {
        found: false, editor_present: Boolean(editor),
        editor_rendered: Boolean(editor) && editor.getClientRects().length > 0 && getComputedStyle(editor).visibility === 'visible',
        here_cards: document.querySelectorAll(`.card.here[data-asset="${CSS.escape(input.asset)}"]`).length,
        focus_viewport: document.querySelector('.focus-viewport') !== null, focus_entering: document.querySelector('.focus-viewport[data-entering]') !== null,
        visible: document.visibilityState === 'visible', has_focus: document.hasFocus(), animation_frame: frame, inert: document.documentElement.inert,
        ...Object.fromEntries(['mousedown', 'click', 'dblclick'].flatMap((type) => [[type, globalThis.__cozyProofClicks?.[type] === true], [`${type}_in_node`, globalThis.__cozyProofClicks?.[`${type}_in_node`] === true]])),
      };
    }
    case 'observe':
      return { visible: document.body.textContent.includes(input.needle), frozen: document.documentElement.inert, images: document.querySelectorAll('img[src]').length };
    case 'hmr_start': {
      const development = [...document.scripts].some((script) => script.src && new URL(script.src).pathname === '/@vite/client');
      if (!development) return { development: false };
      globalThis.__cozyHmrDocument = crypto.randomUUID();
      const module = await import('/src/__cozy_hmr_proof.ts');
      const nonce = document.querySelector('meta[property="csp-nonce"]')?.nonce;
      const inline = [...document.scripts].filter((script) => !script.src && script.textContent.trim());
      const styles = [...document.querySelectorAll('style[data-vite-dev-id]')];
      return { development: true, value: module.value, csp_nonce: Boolean(nonce && nonce !== '__COZY_DEVELOPMENT_NONCE__'), refresh_nonce: inline.length > 0 && inline.every((script) => script.nonce === nonce), style_nonce: styles.length > 0 && styles.every((style) => style.nonce === nonce), document_sha256: await hash(new TextEncoder().encode(globalThis.__cozyHmrDocument)) };
    }
    case 'hmr_observe':
      return { value: globalThis.__cozyHmrProof ?? 0, document_sha256: await hash(new TextEncoder().encode(globalThis.__cozyHmrDocument ?? '')) };
    case 'storage': {
      const keys = Object.keys(localStorage);
      const databases = indexedDB.databases ? await indexedDB.databases() : null;
      return { project_preferences: keys.filter((key) => /^cozy\.(view|selection|more):/.test(key)).length, durable_session: localStorage.getItem('cozy.session') !== null, indexed_databases: databases?.length ?? -1, cache_stores: 'caches' in globalThis ? (await caches.keys()).length : -1, cookies: document.cookie.length };
    }
    case 'media': {
      const owned = globalThis.__cozyOwned;
      if (!owned || typeof owned.session?.token !== 'string') return { authenticated: false };
      const authority = { Authorization: `Bearer ${owned.session.token}` };
      const probe = await fetch('/v1/project-storage', { headers: authority, cache: 'no-store' });
      const cap = probe.ok ? await probe.json() : null;
      if (cap?.version !== 3 || cap.daemon_birth !== owned.birth || cap.principal !== await hash(new TextEncoder().encode(owned.session.token))) return { authenticated: false };
      const path = new URL(input.path, location.origin);
      const prefix = `/v1/projects/${encodeURIComponent(input.project)}/`;
      if (path.origin !== location.origin || !path.pathname.startsWith(prefix)) return { authenticated: false };
      const headers = { ...authority, 'cozy-project-storage': '3' };
      if (input.range) headers.Range = 'bytes=0-31';
      if (input.conditional) headers['If-Modified-Since'] = 'Wed, 01 Jan 2100 00:00:00 GMT';
      const response = await fetch(path, { headers, cache: 'no-store' });
      const bytes = await response.arrayBuffer();
      return { authenticated: true, status: response.status, no_store: /(?:^|,)\s*(?:private,\s*)?no-store(?:\s*,|$)/i.test(response.headers.get('Cache-Control') ?? ''), range: response.headers.has('Content-Range'), body_sha256: await hash(bytes), bytes: bytes.byteLength };
    }
    default:
      throw new Error('unsupported fixed proof action');
  }
}
