/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import { CSP } from './csp';

/** Adds the Content-Security-Policy and a no-referrer policy to the built index.html. */
function securityHeaders(): Plugin {
  return {
    name: 'mvp-security-headers',
    apply: 'build',
    transformIndexHtml: () => [
      { tag: 'meta', attrs: { 'http-equiv': 'Content-Security-Policy', content: CSP }, injectTo: 'head-prepend' },
      { tag: 'meta', attrs: { name: 'referrer', content: 'no-referrer' }, injectTo: 'head-prepend' },
    ],
  };
}

// `base: './'` keeps asset paths relative so the built app loads from
// file:// or a custom scheme inside Tauri or Electron without changes.
export default defineConfig({
  base: './',
  plugins: [react(), securityHeaders()],
  build: {
    // No source maps in the shipped bundle.
    sourcemap: false,
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    // Above the 5 s findBy/waitFor limit in setup.ts, so a slow wait fails as
    // a wait (with its message), not as a test timeout.
    testTimeout: 15_000,
  },
});
