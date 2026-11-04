// Copying text for the user (invite links). In the desktop app the page has
// no clipboard permission, so the shell's `window.mvs.copyText` writes it;
// in a browser, navigator.clipboard. Resolves false when nothing was copied,
// so the UI can say so and the person can copy the shown text by hand.

export async function copyText(text: string): Promise<boolean> {
  const bridge = (window as { mvs?: { copyText?: (t: string) => Promise<boolean> } }).mvs;
  try {
    if (typeof bridge?.copyText === 'function') return (await bridge.copyText(text)) === true;
    if (!navigator.clipboard) return false;
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
