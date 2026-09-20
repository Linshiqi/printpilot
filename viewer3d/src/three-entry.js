// three.js 打包入口:只导出 public/viewer3d/bridge.mjs 用到的东西,其余交给 esbuild 摇树。
// 增减导出后运行 `pnpm build`,并把新的 public/viewer3d/three-bundle.mjs 一起提交。
export {
  Box3,
  Box3Helper,
  Color,
  DirectionalLight,
  DoubleSide,
  EdgesGeometry,
  FrontSide,
  GridHelper,
  Group,
  HemisphereLight,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  PerspectiveCamera,
  Plane,
  PlaneGeometry,
  Raycaster,
  Scene,
  SphereGeometry,
  SRGBColorSpace,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three';
export { OrbitControls } from 'three/addons/controls/OrbitControls.js';
export { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
export { STLLoader } from 'three/addons/loaders/STLLoader.js';
