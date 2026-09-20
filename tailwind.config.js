/** @type {import('tailwindcss').Config} */
// Tailwind v4:配置基本都在 style/input.css 里(@theme / @custom-variant)。
// 这个文件只告诉它去哪里找类名——Rust 源码里的 class="…" 字符串。
export default {
  darkMode: 'selector',
  content: [
    "./src/**/*.{rs,html}",
    "./index.html",
  ],
  theme: {
    extend: {},
  },
  plugins: [],
}
