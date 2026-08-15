import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import tsconfigPaths from 'vite-tsconfig-paths';

const directory = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig(({ command }) => ({
  root: directory,
  base: command === 'build' ? '/app/' : '/',
  plugins: [
    tsconfigPaths({
      projects: [resolve(directory, '../../../../../tsconfig.json')],
    }),
  ],
  define: {
    __CACHE_WASM_BUILD_MODE__: JSON.stringify(
      command === 'build' ? 'production' : 'development'
    ),
  },
  server: {
    host: '127.0.0.1',
    port: 4188,
    strictPort: true,
  },
  preview: {
    host: '127.0.0.1',
    port: 4189,
    strictPort: true,
  },
  worker: { format: 'es' },
  build: {
    target: 'esnext',
    outDir: resolve(directory, '.dist-production'),
    emptyOutDir: true,
    sourcemap: true,
    assetsInlineLimit: 0,
    rollupOptions: {
      input: Object.fromEntries(
        [
          'index.html',
          'host.html',
          'cutover.html',
          'production.html',
          'production-tab.html',
          'tab.html',
          'measurement.html',
          'exact-production-host.html',
          'wp12.html',
          'wp12-graphql-soup.html',
          'wp12-internal.html',
        ].map((name) => [name.replace('.html', ''), resolve(directory, name)])
      ),
    },
  },
}));
