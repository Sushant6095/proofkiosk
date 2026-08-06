import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  publicDir: false,
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // The single interactive 3D runtime is intentionally shared by both scenes.
    chunkSizeWarningLimit: 1100,
    rollupOptions: {
      input: 'docs/index.html',
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return undefined;
          if (id.includes('/three/')) return 'three-core';
          if (id.includes('@react-three')) return 'react-three';
          if (id.includes('/react/') || id.includes('/react-dom/') || id.includes('/scheduler/')) {
            return 'react-vendor';
          }
          return undefined;
        },
      },
    },
  },
});
