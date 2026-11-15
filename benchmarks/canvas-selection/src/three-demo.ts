import type { BufferGeometry, Material, Mesh, Skeleton, Texture, WebGLRenderer } from 'three';
import type { WebGPURenderer } from 'three/webgpu';

export interface ThreeDemo {
  start(): void;
  stop(): void;
  stats(): Record<string, unknown>;
  dispose(): void;
}

/** An independent, transparent 3D canvas. It does not share the overview's GPU resources. */
export async function createThreeDemo(host: HTMLElement, backend: 'webgl' | 'webgpu'): Promise<ThreeDemo> {
  const began = performance.now();
  const THREE = await import('three');
  const [{ GLTFLoader }, { OrbitControls }] = await Promise.all([
    import('three/addons/loaders/GLTFLoader.js'),
    import('three/addons/controls/OrbitControls.js'),
  ]);
  const errors: string[] = [];
  let renderer: WebGLRenderer | WebGPURenderer;
  if (backend === 'webgpu') {
    const { WebGPURenderer } = await import('three/webgpu');
    const gpuRenderer = new WebGPURenderer({ alpha: true, antialias: true, forceWebGL: false });
    try {
      await gpuRenderer.init();
      const actual = gpuRenderer.backend as unknown as { isWebGPUBackend?: boolean };
      if (actual.isWebGPUBackend !== true) throw new Error('Three WebGPU requested, but renderer selected a fallback backend');
    } catch (error) {
      gpuRenderer.dispose();
      throw error;
    }
    renderer = gpuRenderer;
  } else {
    renderer = new THREE.WebGLRenderer({ alpha: true, antialias: true });
  }

  const canvas = renderer.domElement as HTMLCanvasElement;
  canvas.dataset.threeBackend = backend;
  canvas.setAttribute('aria-label', 'Duck glTF model viewer. Drag to orbit; scroll to zoom.');
  Object.assign(canvas.style, { display: 'block', width: '100%', height: '100%', touchAction: 'none' });
  host.appendChild(canvas);
  renderer.setPixelRatio(window.devicePixelRatio || 1);
  renderer.setClearColor(0x000000, 0);
  renderer.outputColorSpace = THREE.SRGBColorSpace;

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(40, 400 / 250, 0.1, 100);
  camera.position.set(3.1, 1.8, 3.8);
  const model = new THREE.Group();
  scene.add(model);
  scene.add(new THREE.HemisphereLight(0xcce1ff, 0x253025, 2.5));
  const key = new THREE.DirectionalLight(0xffffff, 3);
  key.position.set(3, 5, 4);
  scene.add(key);

  // Fixed, reproducible background workload, one point cloud and one draw call.
  const points = new Float32Array(1000 * 3);
  let seed = 0xdecafbad;
  const random = () => ((seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0) / 0x100000000);
  for (let i = 0; i < points.length; i += 3) {
    const angle = random() * Math.PI * 2;
    const vertical = random() * 2 - 1;
    const radius = 8 + random() * 8;
    const horizontal = Math.sqrt(1 - vertical * vertical);
    points[i] = radius * horizontal * Math.cos(angle);
    points[i + 1] = radius * vertical;
    points[i + 2] = radius * horizontal * Math.sin(angle);
  }
  const starsGeometry = new THREE.BufferGeometry();
  starsGeometry.setAttribute('position', new THREE.BufferAttribute(points, 3));
  const starsMaterial = new THREE.PointsMaterial({ color: 0x9ab8dc, size: 0.045, transparent: true, opacity: 0.7, depthWrite: false });
  scene.add(new THREE.Points(starsGeometry, starsMaterial));

  const geometries = new Set<BufferGeometry>([starsGeometry]);
  const materials = new Set<Material>([starsMaterial]);
  const textures = new Set<Texture>();
  const skeletons = new Set<Skeleton>();
  let meshCount = 0;
  let modelVertices = 0;
  let modelTriangles = 0;
  let modelReady = false;
  let modelLoadMs = 0;
  let frames = 0;
  let animatedFrames = 0;
  let framesWithModel = 0;
  let lastRenderMs = 0;
  let totalRenderCallMs = 0;
  let orbitChanges = 0;
  let playing = false;
  let disposed = false;
  let prepared = false;
  let dirty = true;
  let raf = 0;
  let lastAt = 0;
  let width = 400;
  let height = 250;

  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = false; // Paused interaction needs no perpetual update loop.
  controls.autoRotate = false;
  controls.minDistance = 1.7;
  controls.maxDistance = 15;
  controls.target.set(0, 0, 0);
  controls.update();

  function schedule() {
    if (prepared && !disposed && !raf) raf = requestAnimationFrame(frame);
  }
  function invalidate() {
    dirty = true;
    schedule();
  }
  function stop() {
    playing = false;
    lastAt = 0;
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    if (dirty) schedule();
  }
  function failure(where: string, error: unknown) {
    if (disposed) return;
    errors.push(`${where}: ${error instanceof Error ? error.message : String(error)}`);
    dirty = false;
    stop();
  }
  function frame(now: number) {
    raf = 0;
    if (disposed) return;
    if (playing) {
      const delta = lastAt ? Math.max(0, Math.min(100, now - lastAt)) : 0;
      model.rotation.y += delta * 0.0006;
      lastAt = now;
      dirty = true;
    }
    if (dirty) {
      try {
        const before = performance.now();
        renderer.render(scene, camera);
        lastRenderMs = performance.now() - before;
        totalRenderCallMs += lastRenderMs;
        frames++;
        if (playing) animatedFrames++;
        if (modelReady) framesWithModel++;
        dirty = false;
      } catch (error) {
        failure('render', error);
      }
    }
    if (playing) schedule();
  }
  function resize() {
    if (disposed) return;
    const bounds = host.getBoundingClientRect();
    width = Math.max(1, Math.round(bounds.width || 400));
    height = Math.max(1, Math.round(bounds.height || 250));
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    invalidate();
  }
  const stopPropagation = (event: Event) => event.stopPropagation();
  const contextLost = (event: Event) => { event.preventDefault(); failure('WebGL context lost', 'context unavailable'); };
  const controlsChanged = () => { orbitChanges++; invalidate(); };
  controls.addEventListener('change', controlsChanged);
  canvas.addEventListener('pointerdown', stopPropagation);
  // OrbitControls receives drag movement/release through ownerDocument's bubble listeners.
  canvas.addEventListener('wheel', stopPropagation);
  canvas.addEventListener('dblclick', stopPropagation);
  canvas.addEventListener('webglcontextlost', contextLost);
  const observer = new ResizeObserver(resize);
  observer.observe(host);
  resize();

  // Subscribe without replacing Three's own backend error handler.
  const device = backend === 'webgpu'
    ? (renderer as WebGPURenderer).backend as unknown as { device?: EventTarget & { lost: Promise<{ reason: string; message: string }> } }
    : null;
  const gpuError = (event: Event) => failure('WebGPU', (event as Event & { error?: { message?: string } }).error?.message || 'uncaptured device error');
  device?.device?.addEventListener('uncapturederror', gpuError);
  void device?.device?.lost.then(info => {
    if (info.reason !== 'destroyed') failure('WebGPU device lost', info.message || info.reason);
  });

  function dispose() {
    if (disposed) return;
    disposed = true;
    playing = false;
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
    observer.disconnect();
    controls.removeEventListener('change', controlsChanged);
    controls.dispose();
    canvas.removeEventListener('pointerdown', stopPropagation);
    canvas.removeEventListener('wheel', stopPropagation);
    canvas.removeEventListener('dblclick', stopPropagation);
    canvas.removeEventListener('webglcontextlost', contextLost);
    device?.device?.removeEventListener('uncapturederror', gpuError);
    for (const geometry of geometries) geometry.dispose();
    for (const material of materials) material.dispose();
    const closed = new Set<ImageBitmap>();
    for (const texture of textures) {
      texture.dispose();
      const images: unknown[] = Array.isArray(texture.image) ? texture.image : [texture.image];
      for (const image of images) {
        if (typeof ImageBitmap !== 'undefined' && image instanceof ImageBitmap && !closed.has(image)) {
          image.close();
          closed.add(image);
        }
      }
    }
    for (const skeleton of skeletons) skeleton.dispose();
    geometries.clear(); materials.clear(); textures.clear(); skeletons.clear();
    scene.clear();
    renderer.dispose();
    if (backend === 'webgl') (renderer as WebGLRenderer).forceContextLoss();
    canvas.remove();
  }

  try {
    const loadBegan = performance.now();
    try {
      const gltf = await new GLTFLoader().loadAsync('/duck.glb');
      model.add(gltf.scene);
      gltf.scene.traverse(object => {
        const mesh = object as Mesh & { skeleton?: Skeleton };
        if (mesh.geometry) {
          geometries.add(mesh.geometry);
          if (mesh.isMesh) {
            meshCount++;
            modelVertices += mesh.geometry.getAttribute('position')?.count || 0;
            modelTriangles += (mesh.geometry.index?.count || mesh.geometry.getAttribute('position')?.count || 0) / 3;
          }
        }
        if (mesh.skeleton) skeletons.add(mesh.skeleton);
        const owned = Array.isArray(mesh.material) ? mesh.material : mesh.material ? [mesh.material] : [];
        for (const material of owned) {
          materials.add(material);
          for (const value of Object.values(material)) {
            if (value && typeof value === 'object' && (value as Texture).isTexture) textures.add(value as Texture);
          }
        }
      });
      const bounds = new THREE.Box3().setFromObject(gltf.scene);
      const size = bounds.getSize(new THREE.Vector3());
      const extent = Math.max(size.x, size.y, size.z);
      if (!Number.isFinite(extent) || extent <= 0) throw new Error('Duck GLB has no finite nonempty bounds');
      gltf.scene.scale.multiplyScalar(2.2 / extent);
      const center = new THREE.Box3().setFromObject(gltf.scene).getCenter(new THREE.Vector3());
      gltf.scene.position.sub(center);
      modelReady = meshCount > 0;
      if (!modelReady) throw new Error('Duck GLB loaded without a mesh');
    } catch (error) {
      errors.push(`model load: ${error instanceof Error ? error.message : String(error)}`);
    }
    modelLoadMs = performance.now() - loadBegan;
    await renderer.compileAsync(scene, camera);
    prepared = true;
    frame(performance.now());
  } catch (error) {
    dispose();
    throw error;
  }

  const initializationMs = performance.now() - began;
  return {
    start() {
      if (disposed || playing || !modelReady || errors.length) return;
      playing = true;
      lastAt = 0;
      schedule();
    },
    stop,
    stats() {
      const info = renderer.info as unknown as {
        memory?: Record<string, number>;
        render?: Record<string, number>;
        programs?: unknown[];
      };
      return {
        requestedBackend: backend,
        actualBackend: backend === 'webgpu' ? 'Three WebGPUBackend' : 'Three WebGLRenderer',
        separateCanvas: true, transparent: true, sharesOverviewResources: false,
        threeRevision: THREE.REVISION,
        modelUrl: new URL('/duck.glb', location.href).href,
        modelReady, modelRendered: framesWithModel > 0, modelLoadMs, meshCount, modelVertices, modelTriangles,
        backgroundPoints: 1000,
        width, height, pixelRatio: renderer.getPixelRatio(),
        initializationMs, frames, animatedFrames, framesWithModel,
        lastRenderCallMs: lastRenderMs, totalRenderCallMs,
        orbitChanges, cameraPosition: camera.position.toArray(), orbitTarget: controls.target.toArray(),
        playing, disposed, rafPending: raf !== 0, dirty,
        errors: [...errors],
        ownedResources: { geometries: geometries.size, materials: materials.size, textures: textures.size, skeletons: skeletons.size },
        rendererInfo: { memory: { ...info.memory }, render: { ...info.render }, programs: info.programs?.length },
      };
    },
    dispose,
  };
}
