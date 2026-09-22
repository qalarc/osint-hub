import { defineConfig, type Plugin } from 'vite';

/**
 * esbuild keeps `/*! ... *\/` legal banners at EOF of the minified bundle.
 * Some upstream banners contain http URLs; the workbench must be fully
 * self-contained (zero external references), so strip them post-render.
 * (Upstream licenses: cytoscape.js MIT, cytoscape-fcose MIT — see NOTICE.)
 */
const stripLegalBanners = (): Plugin => ({
  name: 'strip-legal-banners',
  enforce: 'post',
  generateBundle(_opts, bundle) {
    for (const file of Object.values(bundle)) {
      if (file.type === 'chunk' && typeof file.code === 'string') {
        file.code = file.code.replace(/\/\*![\s\S]*?\*\//g, '');
      }
    }
  },
});

export default defineConfig({
  base: './',
  plugins: [stripLegalBanners()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2020',
    cssCodeSplit: false,
    assetsInlineLimit: 0,
    sourcemap: false,
    reportCompressedSize: false,
  },
  esbuild: {
    legalComments: 'none',
  },
  server: {
    port: 5175,
    strictPort: false,
  },
});
