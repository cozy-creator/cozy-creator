async function containmentProof(input) {
  // Fixed hosted proof actions only. No caller-selected JavaScript or replacement app/client.
  const hash = async (bytes) => [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map((n) => n.toString(16).padStart(2, '0')).join('');
  const card = () => document.querySelector(`.card.here[data-asset="${CSS.escape(input.asset)}"]`) ?? document.querySelector(`.react-flow__node .card[data-asset="${CSS.escape(input.asset)}"]`);
  switch (input.kind) {
    case 'ready':
      return { page_ready: typeof globalThis.__cozyFreeze === 'function' && typeof globalThis.__cozyPrepareClose === 'function', cards: document.querySelectorAll('.card[data-asset]').length };
    case 'locate': {
      const box = card(), target = box?.classList.contains('here') ? box : box?.querySelector('.media');
      if (!target) return { found: false };
      const rect = target.getBoundingClientRect();
      return { found: rect.width > 0 && rect.height > 0, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
    }
    case 'focus': {
      const editor = card()?.querySelector(`.mention-text[data-field="${CSS.escape(input.field)}"]`);
      if (!editor) return { found: false };
      editor.focus();
      return { found: document.activeElement === editor };
    }
    case 'observe':
      return { visible: document.body.textContent.includes(input.needle), frozen: document.documentElement.inert, images: document.querySelectorAll('img[src]').length };
    case 'hmr_start': {
      const development = [...document.scripts].some((script) => script.src && new URL(script.src).pathname === '/@vite/client');
      if (!development) return { development: false };
      globalThis.__cozyHmrDocument = crypto.randomUUID();
      const module = await import('/src/__cozy_hmr_proof.ts');
      return { development: true, value: module.value, document_sha256: await hash(new TextEncoder().encode(globalThis.__cozyHmrDocument)) };
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
