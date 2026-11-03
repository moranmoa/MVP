// Content-Security-Policy for the built UI. Vite adds it to index.html as a
// <meta> tag on `vite build` (the dev server needs inline scripts for hot
// reload, so it runs without one). The desktop shell should send the same
// policy itself; SECURITY.md has the Tauri and Electron settings.
//
// Nothing here loads from the network: messages and images reach the page only
// through the core, as data. React sets inline styles through the CSSOM, which
// `style-src 'self'` allows, so no 'unsafe-inline' is needed.

export const CSP_DIRECTIVES: Record<string, string[]> = {
  'default-src': ["'none'"],
  'script-src': ["'self'"],
  'style-src': ["'self'"],
  // Avatars are inline data: URLs (checked in src/core/sanitize.ts); blob: for future file previews.
  'img-src': ["'self'", 'data:', 'blob:'],
  'font-src': ["'self'"],
  // The Tauri shell adds its IPC origin here (`ipc: http://ipc.localhost`).
  'connect-src': ["'self'"],
  'media-src': ["'self'", 'blob:'],
  'manifest-src': ["'self'"],
  'worker-src': ["'self'"],
  'object-src': ["'none'"],
  'base-uri': ["'none'"],
  'form-action': ["'none'"],
};

export const CSP = Object.entries(CSP_DIRECTIVES)
  .map(([k, v]) => `${k} ${v.join(' ')}`)
  .join('; ');
