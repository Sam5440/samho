import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// 产物固定文件名（web/dist/index.html + assets/app.{js,css}），由 server.rs 的
// STATIC_ASSETS 通过 include_str! 编译进二进制；不做代码分割，避免哈希文件名。
// CSP 为 style-src 'self' / script-src 'self'：不得依赖运行期注入 <style> 或内联脚本。
const target = process.env.SAMHO_DEV_API || 'http://127.0.0.1:7432';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      // Radix 的滚动锁会注入 <style>（CSP 拒绝），替换为透传垫片。
      'react-remove-scroll': fileURLToPath(new URL('./src/shims/react-remove-scroll.tsx', import.meta.url)),
    },
  },
  base: '/',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    modulePreload: false,
    cssCodeSplit: false,
    assetsInlineLimit: 0,
    sourcemap: false,
    rolldownOptions: {
      output: {
        entryFileNames: 'assets/app.js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: (info) => (info.names?.some((n) => n.endsWith('.css')) ? 'assets/app.css' : 'assets/[name][extname]'),
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': { target, changeOrigin: false },
      '/vendor': { target },
      '/w/': { target },
    },
  },
});
