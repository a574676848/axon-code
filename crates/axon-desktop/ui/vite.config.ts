import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// https://vite.dev/config/
export default defineConfig({
  base: './',
  plugins: [
    react(),
    tailwindcss(),
  ],
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'esnext',
    outDir: 'dist',
    emptyOutDir: true,
  },
});
