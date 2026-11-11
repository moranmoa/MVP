// B11e §4: a picked profile image becomes a small WebP data URL before it goes
// to updateProfile. A big photo is scaled down, never refused for its size;
// only something too big to decode safely is refused.

import { safeImageSrc } from '../core/sanitize';

export const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
/** Decode ceiling, so processing can't exhaust memory (approved update to ש-22). */
const MAX_FILE_BYTES = 25 * 1024 * 1024;
const MAX_PIXELS = 40_000_000;
const AVATAR_SIZE = 256;
const AVATAR_MAX_BYTES = 512 * 1024;

export type ImageError = 'too-large' | 'unsupported';
export type ImageResult = { ok: true; dataUrl: string } | { ok: false; error: ImageError };

function readAsDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsDataURL(blob);
  });
}

/** Can this browser scale images (createImageBitmap and OffscreenCanvas)? */
function canScale(): boolean {
  return typeof createImageBitmap === 'function' && typeof OffscreenCanvas === 'function';
}

/** A square avatar: 256×256 from the centre (smaller images aren't enlarged), WebP. */
export async function avatarFromFile(file: File): Promise<ImageResult> {
  if (!IMAGE_TYPES.includes(file.type)) return { ok: false, error: 'unsupported' };
  if (file.size > MAX_FILE_BYTES) return { ok: false, error: 'too-large' };
  if (!canScale()) {
    // No canvas (tests, old engines): take a small file as it is.
    if (file.size > AVATAR_MAX_BYTES) return { ok: false, error: 'too-large' };
    const src = safeImageSrc(await readAsDataUrl(file).catch(() => ''));
    return src ? { ok: true, dataUrl: src } : { ok: false, error: 'unsupported' };
  }
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file); // a GIF gives its first frame
  } catch {
    return { ok: false, error: 'unsupported' };
  }
  try {
    if (bitmap.width * bitmap.height > MAX_PIXELS) return { ok: false, error: 'too-large' };
    const side = Math.min(bitmap.width, bitmap.height);
    const out = Math.min(AVATAR_SIZE, side);
    const canvas = new OffscreenCanvas(out, out);
    const ctx = canvas.getContext('2d');
    if (!ctx) return { ok: false, error: 'unsupported' };
    ctx.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, out, out);
    let blob = await canvas.convertToBlob({ type: 'image/webp', quality: 0.85 });
    if (blob.size > AVATAR_MAX_BYTES) blob = await canvas.convertToBlob({ type: 'image/webp', quality: 0.7 });
    const src = safeImageSrc(await readAsDataUrl(blob));
    return src ? { ok: true, dataUrl: src } : { ok: false, error: 'unsupported' };
  } finally {
    bitmap.close();
  }
}

/**
 * #359: the 48×48 picture others see: WebP (PNG where WebP isn't available),
 * lowering the quality until it's 2,560 bytes or less. Re-encoding through the
 * canvas drops any EXIF. Undefined when it can't be made small enough.
 */
export async function smallAvatarFrom(dataUrl: string): Promise<string | undefined> {
  if (typeof createImageBitmap !== 'function' || typeof OffscreenCanvas !== 'function') return undefined;
  try {
    const bitmap = await createImageBitmap(await (await fetch(dataUrl)).blob());
    const side = Math.min(bitmap.width, bitmap.height);
    const canvas = new OffscreenCanvas(48, 48);
    canvas.getContext('2d')!.drawImage(bitmap, (bitmap.width - side) / 2, (bitmap.height - side) / 2, side, side, 0, 0, 48, 48);
    bitmap.close();
    for (const quality of [0.8, 0.6, 0.4, 0.25]) {
      const blob = await canvas.convertToBlob({ type: 'image/webp', quality });
      if (blob.size <= 2560) return safeImageSrc(await readAsDataUrl(blob));
    }
    const png = await canvas.convertToBlob({ type: 'image/png' });
    return png.size <= 2560 ? safeImageSrc(await readAsDataUrl(png)) : undefined;
  } catch {
    return undefined;
  }
}
