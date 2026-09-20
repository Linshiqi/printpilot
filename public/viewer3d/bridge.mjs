// three.js 视图桥(模式照搬 velo 嵌 PDF.js 的 public/pdfjs/bridge.mjs)。
//
// 分工:**这里只负责渲染**。工具栏、参数面板、主题、i18n 全在 Leptos;状态是 Leptos 的信号,
// 这里是被动执行者。几何运算(缩放、切平底…)在 Rust 后端的 pp-geometry 里做,这里只做视觉预览。
//
// 三条来自 velo 的纪律:
// 1. 本文件经 `#[wasm_bindgen(module = …)]` 引入后会被拷到 dist/snippets/<crate>/ 下,
//    所以**引用其它资源一律写绝对路径 `/public/...`**——相对路径会解析到 snippets 目录,404。
// 2. three.js 用动态 `import()` 懒加载:主界面不为 3D 付加载成本,第一次打开 3D 视图才取。
//    动态 import 是语言特性,不触发 CSP 的 unsafe-eval。
// 3. 高分屏按 devicePixelRatio 放大画布,但**上限 2**——再高只是烧显存和填充率。

const THREE_BUNDLE = '/public/viewer3d/three-bundle.mjs';

let lib = null;
async function three() {
  if (!lib) lib = await import(THREE_BUNDLE);
  return lib;
}

/** @type {Map<string, any>} 容器 id → 视图实例 */
const views = new Map();

const BED = 256; // 成型范围参考框的边长(mm);以后由 Leptos 按打印机档案传入

function palette(dark) {
  return dark
    ? { bg: 0x111827, grid: 0x374151, gridMajor: 0x4b5563, bed: 0x6366f1, model: 0xa5b4fc, cut: 0xf59e0b, edge: 0x1e1b4b, pick: 0xf59e0b }
    : { bg: 0xf3f4f6, grid: 0xd1d5db, gridMajor: 0x9ca3af, bed: 0x6366f1, model: 0x818cf8, cut: 0xf59e0b, edge: 0x312e81, pick: 0xd97706 };
}

export async function mount(containerId, optsJson) {
  const T = await three();
  const el = document.getElementById(containerId);
  if (!el) throw new Error(`viewer container #${containerId} not found`);
  dispose(containerId);
  const opts = JSON.parse(optsJson || '{}');
  const colors = palette(!!opts.dark);

  const renderer = new T.WebGLRenderer({ antialias: true, alpha: false, powerPreference: 'high-performance' });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.localClippingEnabled = true;
  renderer.outputColorSpace = T.SRGBColorSpace;
  renderer.domElement.style.display = 'block';
  renderer.domElement.style.width = '100%';
  renderer.domElement.style.height = '100%';
  el.appendChild(renderer.domElement);

  const scene = new T.Scene();
  scene.background = new T.Color(colors.bg);

  // 与切片软件一致:Z 朝上
  const camera = new T.PerspectiveCamera(40, 1, 0.1, 5000);
  camera.up.set(0, 0, 1);
  camera.position.set(220, -260, 200);

  const controls = new T.OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.12;
  controls.target.set(0, 0, 40);

  scene.add(new T.HemisphereLight(0xffffff, 0x8d8d9c, 1.1));
  const key = new T.DirectionalLight(0xffffff, 1.6);
  key.position.set(200, -300, 400);
  scene.add(key);
  const fill = new T.DirectionalLight(0xffffff, 0.6);
  fill.position.set(-300, 200, 150);
  scene.add(fill);

  // 打印床:网格 + 成型范围线框
  const bed = new T.Group();
  const grid = new T.GridHelper(BED, BED / 16, colors.gridMajor, colors.grid);
  grid.rotation.x = Math.PI / 2; // GridHelper 默认在 XZ 平面,转到 XY
  bed.add(grid);
  const box = new T.Box3(new T.Vector3(-BED / 2, -BED / 2, 0), new T.Vector3(BED / 2, BED / 2, BED));
  const boxHelper = new T.Box3Helper(box, colors.bed);
  boxHelper.material.transparent = true;
  boxHelper.material.opacity = 0.35;
  bed.add(boxHelper);
  scene.add(bed);

  // 切面预览:一个裁剪平面(保留 z ≥ h)+ 一张半透明的指示面
  const cutPlane = new T.Plane(new T.Vector3(0, 0, 1), 0);
  const cutSheet = new T.Mesh(
    new T.PlaneGeometry(1, 1),
    new T.MeshBasicMaterial({ color: colors.cut, transparent: true, opacity: 0.22, side: T.DoubleSide, depthWrite: false }),
  );
  cutSheet.visible = false;
  scene.add(cutSheet);

  const view = {
    T, el, renderer, scene, camera, controls, bed, cutPlane, cutSheet, colors,
    model: null, wireframe: false, cutEnabled: false,
    edgesEnabled: false, edges: null, pickEnabled: false, pickHandler: null, pickMarker: null, pickCleanup: null,
    frames: 0, fps: 0, fpsStamp: performance.now(), raf: 0, loadMs: 0, triangles: 0,
    observer: null,
  };
  view.pickCleanup = wirePicking(view);

  const resize = () => {
    const w = Math.max(el.clientWidth, 1);
    const h = Math.max(el.clientHeight, 1);
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  };
  view.observer = new ResizeObserver(resize);
  view.observer.observe(el);
  resize();

  const tick = (now) => {
    view.raf = requestAnimationFrame(tick);
    controls.update();
    renderer.render(scene, camera);
    view.frames += 1;
    if (now - view.fpsStamp >= 1000) {
      view.fps = Math.round((view.frames * 1000) / (now - view.fpsStamp));
      view.frames = 0;
      view.fpsStamp = now;
    }
  };
  view.raf = requestAnimationFrame(tick);

  views.set(containerId, view);
  return 'ok';
}

function clearEdges(view) {
  if (!view.edges) return;
  view.scene.remove(view.edges);
  view.edges.geometry.dispose();
  view.edges.material.dispose();
  view.edges = null;
}

/** 棱线叠加:CAD 零件的孔、槽、倒角靠棱线才看得清(对人、对看图复核的视觉模型都是)。第一次打开时才算。 */
function applyEdges(view) {
  if (!view.edgesEnabled || !view.model) {
    if (view.edges) view.edges.visible = false;
    return;
  }
  if (!view.edges) {
    const T = view.T;
    let source = null;
    view.model.traverse((o) => { if (!source && o.isMesh && o.geometry) source = o; });
    if (!source) return;
    // 25°:曲面上相邻三角形之间的小折角不算棱,真正的特征边才画出来
    const geometry = new T.EdgesGeometry(source.geometry, 25);
    const lines = new T.LineSegments(geometry, new T.LineBasicMaterial({ color: view.colors.edge }));
    source.updateWorldMatrix(true, false);
    lines.applyMatrix4(source.matrixWorld);
    view.edges = lines;
    view.scene.add(lines);
  }
  view.edges.visible = true;
}

function clearPickMarker(view) {
  if (!view.pickMarker) return;
  view.scene.remove(view.pickMarker);
  view.pickMarker.geometry.dispose();
  view.pickMarker.material.dispose();
  view.pickMarker = null;
}

/**
 * 点选:在模型上点一下 → 回调 JSON {point:[x,y,z], normal:[nx,ny,nz]}(模型坐标系,毫米,Z 朝上)。
 * 按下和抬起之间移动超过 4 像素算拖拽(转视角),不算点选。
 */
function wirePicking(view) {
  const dom = view.renderer.domElement;
  let down = null;
  const onDown = (e) => { down = { x: e.clientX, y: e.clientY }; };
  const onUp = (e) => {
    const start = down;
    down = null;
    if (!view.pickEnabled || !view.model || !start || e.button !== 0) return;
    if (Math.hypot(e.clientX - start.x, e.clientY - start.y) > 4) return;
    const T = view.T;
    const rect = dom.getBoundingClientRect();
    const ndc = new T.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    const ray = new T.Raycaster();
    ray.setFromCamera(ndc, view.camera);
    const hit = ray.intersectObject(view.model, true)[0];
    if (!hit) return;
    const normal = hit.face
      ? hit.face.normal.clone().transformDirection(hit.object.matrixWorld)
      : new T.Vector3(0, 0, 1);
    showPickMarker(view, hit.point);
    const round = (v) => Math.round(v * 100) / 100;
    const payload = { point: hit.point.toArray().map(round), normal: normal.toArray().map(round) };
    if (view.pickHandler) view.pickHandler(JSON.stringify(payload));
  };
  dom.addEventListener('pointerdown', onDown);
  dom.addEventListener('pointerup', onUp);
  return () => {
    dom.removeEventListener('pointerdown', onDown);
    dom.removeEventListener('pointerup', onUp);
  };
}

function showPickMarker(view, point) {
  const T = view.T;
  clearPickMarker(view);
  const size = view.bounds ? view.bounds.getSize(new T.Vector3()).length() : 100;
  const marker = new T.Mesh(
    new T.SphereGeometry(Math.max(size * 0.012, 0.4), 20, 12),
    new T.MeshBasicMaterial({ color: view.colors.pick, depthTest: false, transparent: true, opacity: 0.95 }),
  );
  marker.renderOrder = 10;
  marker.position.copy(point);
  view.pickMarker = marker;
  view.scene.add(marker);
}

function clearModel(view) {
  clearEdges(view);
  clearPickMarker(view);
  if (!view.model) return;
  view.scene.remove(view.model);
  view.model.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    const mats = Array.isArray(o.material) ? o.material : o.material ? [o.material] : [];
    for (const m of mats) {
      for (const k of Object.keys(m)) if (m[k] && m[k].isTexture) m[k].dispose();
      m.dispose();
    }
  });
  view.model = null;
}

function applyMaterialState(view) {
  if (!view.model) return;
  view.model.traverse((o) => {
    if (!o.isMesh) return;
    const mats = Array.isArray(o.material) ? o.material : [o.material];
    for (const m of mats) {
      m.wireframe = view.wireframe;
      m.clippingPlanes = view.cutEnabled ? [view.cutPlane] : [];
      m.side = view.cutEnabled ? view.T.DoubleSide : view.T.FrontSide; // 切开后要能看到内壁
      m.needsUpdate = true;
    }
  });
}

function frame(view) {
  const T = view.T;
  const bounds = new T.Box3().setFromObject(view.model);
  const size = bounds.getSize(new T.Vector3());
  const center = bounds.getCenter(new T.Vector3());
  const radius = Math.max(size.length() / 2, 1);
  const dist = radius / Math.sin((view.camera.fov * Math.PI) / 360) * 1.15;
  const dir = new T.Vector3(0.62, -0.72, 0.5).normalize();
  view.camera.position.copy(center).addScaledVector(dir, dist);
  view.camera.near = Math.max(dist / 500, 0.01);
  view.camera.far = dist * 20 + BED * 4;
  view.camera.updateProjectionMatrix();
  view.controls.target.copy(center);
  view.controls.update();
  view.bounds = bounds;
  positionCutSheet(view);
}

function positionCutSheet(view) {
  if (!view.bounds) return;
  const T = view.T;
  const size = view.bounds.getSize(new T.Vector3());
  const center = view.bounds.getCenter(new T.Vector3());
  const h = view.bounds.min.z + (view.cutHeight || 0);
  view.cutSheet.scale.set(size.x * 1.3 + 10, size.y * 1.3 + 10, 1);
  view.cutSheet.position.set(center.x, center.y, h);
  view.cutPlane.constant = -h; // n·p + c ≥ 0  ⇔  z ≥ h
}

/**
 * 加载模型。`url` 是 pp-asset 协议地址;返回 JSON:{ loadMs, triangles }。
 * `keepView`:保持当前视角不动(改参数后重新加载同一个零件时用——每改一次就把镜头弹回去很烦)。
 */
export async function loadModel(containerId, url, format, keepView) {
  const view = views.get(containerId);
  if (!view) throw new Error('viewer not mounted');
  const T = view.T;
  const started = performance.now();
  const hadModel = !!view.model;
  clearModel(view);

  let object;
  if (format === 'glb') {
    const gltf = await new T.GLTFLoader().loadAsync(url);
    object = gltf.scene;
    object.rotation.x = Math.PI / 2; // glTF 是 Y 朝上 → Z 朝上(与后端 y_up_to_z_up 一致)
  } else if (format === 'stl') {
    const geometry = await new T.STLLoader().loadAsync(url);
    const material = new T.MeshStandardMaterial({ color: view.colors.model, roughness: 0.72, metalness: 0.04 });
    object = new T.Mesh(geometry, material);
  } else {
    throw new Error(`unsupported preview format: ${format}`);
  }
  // 如果加载期间视图已经被销毁或又切了模型,丢弃这次结果
  if (views.get(containerId) !== view) return '{}';

  let triangles = 0;
  object.traverse((o) => {
    if (!o.isMesh || !o.geometry) return;
    const g = o.geometry;
    triangles += g.index ? g.index.count / 3 : g.attributes.position.count / 3;
  });

  view.model = object;
  view.scene.add(object);
  applyMaterialState(view);
  if (keepView && hadModel && view.bounds) {
    // 视角方向不动;镜头跟着零件走:目标点随包围盒中心平移,距离按零件大小等比缩放。
    // 只改了槽数之类(包围盒没变)→ 画面纹丝不动;把长度从 60 拉到 90 → 镜头相应退后,零件不会撑出画面。
    const next = new T.Box3().setFromObject(object);
    const oldCenter = view.bounds.getCenter(new T.Vector3());
    const newCenter = next.getCenter(new T.Vector3());
    const oldRadius = Math.max(view.bounds.getSize(new T.Vector3()).length() / 2, 1e-6);
    const newRadius = Math.max(next.getSize(new T.Vector3()).length() / 2, 1e-6);
    const offset = view.camera.position.clone().sub(view.controls.target).multiplyScalar(newRadius / oldRadius);
    view.controls.target.add(newCenter.sub(oldCenter));
    view.camera.position.copy(view.controls.target).add(offset);
    const dist = offset.length();
    view.camera.near = Math.max(dist / 500, 0.01);
    view.camera.far = dist * 20 + BED * 4;
    view.camera.updateProjectionMatrix();
    view.controls.update();
    view.bounds = next;
    positionCutSheet(view);
  } else {
    frame(view);
  }
  applyEdges(view);
  view.loadMs = Math.round(performance.now() - started);
  view.triangles = Math.round(triangles);
  return JSON.stringify({ loadMs: view.loadMs, triangles: view.triangles });
}

export function setDisplay(containerId, wireframe, showBed) {
  const view = views.get(containerId);
  if (!view) return;
  view.wireframe = !!wireframe;
  view.bed.visible = !!showBed;
  applyMaterialState(view);
}

/** 切平底的视觉预览:`height` 是从模型最低点往上的毫米数。真正的切割由后端做。 */
export function setCutPlane(containerId, enabled, height) {
  const view = views.get(containerId);
  if (!view) return;
  view.cutEnabled = !!enabled;
  view.cutHeight = Number.isFinite(height) ? height : 0;
  view.cutSheet.visible = view.cutEnabled && !!view.model;
  positionCutSheet(view);
  applyMaterialState(view);
}

export function setDark(containerId, dark) {
  const view = views.get(containerId);
  if (!view) return;
  view.colors = palette(!!dark);
  view.scene.background = new view.T.Color(view.colors.bg);
  if (view.edges) view.edges.material.color.set(view.colors.edge);
}

/** 返回 JSON:{ fps, loadMs, triangles }。 */
export function stats(containerId) {
  const view = views.get(containerId);
  if (!view) return '{}';
  return JSON.stringify({ fps: view.fps, loadMs: view.loadMs, triangles: view.triangles });
}

/** 当前画面的 PNG(data URL),做缩略图用。先补渲染一帧,否则缓冲区可能已被清空。 */
export function snapshot(containerId) {
  const view = views.get(containerId);
  if (!view) return '';
  view.renderer.render(view.scene, view.camera);
  return view.renderer.domElement.toDataURL('image/png');
}

export function setEdges(containerId, enabled) {
  const view = views.get(containerId);
  if (!view) return;
  view.edgesEnabled = !!enabled;
  applyEdges(view);
}

/** 打开 / 关闭点选。`handler(json)` 在每次点中模型时被调用;关掉时清除标记。 */
export function setPickMode(containerId, enabled, handler) {
  const view = views.get(containerId);
  if (!view) return;
  view.pickEnabled = !!enabled;
  view.pickHandler = enabled ? handler : null;
  view.renderer.domElement.style.cursor = enabled ? 'crosshair' : '';
  if (!enabled) clearPickMarker(view);
}

export function clearPick(containerId) {
  const view = views.get(containerId);
  if (view) clearPickMarker(view);
}

/**
 * 看图复核用的多视角截图:等轴测 + 正面 + 顶面 + 右侧,白底、带棱线、不带打印床。
 * 返回 JSON 数组(JPEG data URL)。全程同步完成,浏览器来不及把中间状态画到屏幕上,所以不会闪。
 */
export function snapshotViews(containerId, size) {
  const view = views.get(containerId);
  if (!view || !view.model) return '[]';
  const T = view.T;
  const S = Math.max(256, Math.min(size || 768, 1024));
  const { renderer, camera, scene } = view;

  const saved = {
    pos: camera.position.clone(), quat: camera.quaternion.clone(), aspect: camera.aspect, near: camera.near, far: camera.far,
    target: view.controls.target.clone(), bg: scene.background, bed: view.bed.visible, cut: view.cutSheet.visible,
    ratio: renderer.getPixelRatio(), size: renderer.getSize(new T.Vector2()),
    wireframe: view.wireframe, cutEnabled: view.cutEnabled, edgesEnabled: view.edgesEnabled,
    marker: view.pickMarker ? view.pickMarker.visible : false,
  };

  view.bed.visible = false;
  view.cutSheet.visible = false;
  if (view.pickMarker) view.pickMarker.visible = false;
  view.wireframe = false;
  view.cutEnabled = false;
  view.edgesEnabled = true;
  applyMaterialState(view);
  applyEdges(view);
  scene.background = new T.Color(0xffffff);
  renderer.setPixelRatio(1);
  renderer.setSize(S, S, false);
  camera.aspect = 1;

  const bounds = new T.Box3().setFromObject(view.model);
  const center = bounds.getCenter(new T.Vector3());
  const radius = Math.max(bounds.getSize(new T.Vector3()).length() / 2, 1);
  const dist = (radius / Math.sin((camera.fov * Math.PI) / 360)) * 1.08;
  camera.near = Math.max(dist / 500, 0.01);
  camera.far = dist * 20;
  camera.updateProjectionMatrix();

  // 顶视图不能正好沿 Z 看(相机的 up 就是 Z,会退化),稍微偏一点
  const dirs = [[0.62, -0.72, 0.5], [0, -1, 0.12], [0, -0.1, 1], [1, 0, 0.12]];
  const shots = [];
  for (const d of dirs) {
    camera.position.copy(center).addScaledVector(new T.Vector3(...d).normalize(), dist);
    camera.lookAt(center);
    renderer.render(scene, camera);
    shots.push(renderer.domElement.toDataURL('image/jpeg', 0.9));
  }

  scene.background = saved.bg;
  view.bed.visible = saved.bed;
  view.cutSheet.visible = saved.cut;
  if (view.pickMarker) view.pickMarker.visible = saved.marker;
  view.wireframe = saved.wireframe;
  view.cutEnabled = saved.cutEnabled;
  view.edgesEnabled = saved.edgesEnabled;
  applyMaterialState(view);
  applyEdges(view);
  renderer.setPixelRatio(saved.ratio);
  renderer.setSize(saved.size.x, saved.size.y, false);
  camera.aspect = saved.aspect;
  camera.near = saved.near;
  camera.far = saved.far;
  camera.position.copy(saved.pos);
  camera.quaternion.copy(saved.quat);
  camera.updateProjectionMatrix();
  view.controls.target.copy(saved.target);
  view.controls.update();
  renderer.render(scene, camera);
  return JSON.stringify(shots);
}

export function dispose(containerId) {
  const view = views.get(containerId);
  if (!view) return;
  views.delete(containerId);
  cancelAnimationFrame(view.raf);
  if (view.observer) view.observer.disconnect();
  if (view.pickCleanup) view.pickCleanup();
  view.pickHandler = null;
  clearModel(view);
  view.controls.dispose();
  view.renderer.dispose();
  view.renderer.forceContextLoss();
  view.renderer.domElement.remove();
}
