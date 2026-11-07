import { defineConfig } from 'vite';

// base 固定为根路径：前端只支持部署在根下（docs/13-deployment.md 的三条纪律）。
// 开发时把 gRPC 路径代理到 daemon，页面因此永远打相对地址，不写死主机。
export default defineConfig({
  base: '/',
  build: { outDir: 'dist' },
  server: {
    proxy: {
      '/openape.v1.OpenApe': {
        target: 'http://127.0.0.1:47711',
        changeOrigin: false,
      },
    },
  },
});
