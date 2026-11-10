// Checks that Windows binaries we ship don't need the VC++ runtime;
// dependency-free (no dumpbin / Visual Studio needed).
//
//   node pe-imports.mjs <ours: file|dir>... [--third-party <file|dir>...]
//
// Reads each PE file's import and delay-import tables. vcruntime*.dll and
// msvcp*.dll come only with the VC++ Redistributable, which a clean Windows
// install does not have (v0.3.0: "VCRUNTIME140.dll was not found"): no file
// may import them. Our own binaries (before --third-party) must not import
// the api-ms-win-crt-* (UCRT) set either: any C runtime import there means
// +crt-static regressed. Files after --third-party (Electron's, e.g. dxil.dll)
// may use the UCRT, which is part of Windows 10 and later (Electron's minimum).
// A directory is searched for .exe/.dll.
// Prints every file's imports so the build log shows what was checked.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const REDIST = /^(vcruntime\d*(_\d+)?|msvcp\d+(_\w+)?)\.dll$/i;
const UCRT = /^api-ms-win-crt-[\w-]+\.dll$/i;

/** DLL names a PE file imports (normal and delay-load). */
function peImports(buf) {
  const u16 = (o) => buf.readUInt16LE(o);
  const u32 = (o) => buf.readUInt32LE(o);
  if (buf.length < 64 || u16(0) !== 0x5a4d) throw new Error("not a PE file (no MZ)");
  const pe = u32(0x3c);
  if (u32(pe) !== 0x4550) throw new Error("not a PE file (no PE signature)");
  const sections = u16(pe + 6);
  const optSize = u16(pe + 20);
  const opt = pe + 24;
  const magic = u16(opt);
  const dirs = opt + (magic === 0x20b ? 112 : magic === 0x10b ? 96 : NaN);
  if (Number.isNaN(dirs)) throw new Error(`unknown optional header magic 0x${magic.toString(16)}`);
  const secTable = opt + optSize;
  const toOffset = (rva) => {
    for (let i = 0; i < sections; i++) {
      const s = secTable + i * 40;
      const va = u32(s + 12), size = Math.max(u32(s + 8), u32(s + 16)), raw = u32(s + 20);
      if (rva >= va && rva < va + size) return rva - va + raw;
    }
    throw new Error(`RVA 0x${rva.toString(16)} is in no section`);
  };
  const cstr = (rva) => {
    const o = toOffset(rva);
    const end = buf.indexOf(0, o);
    return buf.toString("latin1", o, end);
  };
  const names = [];
  // Data directory 1: imports, 20-byte descriptors, Name at +12.
  const imp = u32(dirs + 8);
  if (imp) {
    for (let o = toOffset(imp); ; o += 20) {
      if (u32(o) === 0 && u32(o + 12) === 0 && u32(o + 16) === 0) break;
      names.push(cstr(u32(o + 12)));
    }
  }
  // Data directory 13: delay imports, 32-byte descriptors, DllName at +4.
  const delay = u32(dirs + 13 * 8);
  if (delay) {
    for (let o = toOffset(delay); ; o += 32) {
      if (u32(o + 4) === 0) break;
      names.push(cstr(u32(o + 4)));
    }
  }
  return names;
}

function files(arg) {
  if (!statSync(arg).isDirectory()) return [arg];
  return readdirSync(arg, { recursive: true })
    .map((f) => path.join(arg, f))
    .filter((f) => /\.(exe|dll)$/i.test(f) && statSync(f).isFile());
}

{
  const args = process.argv.slice(2);
  const cut = args.indexOf("--third-party");
  const ours = (cut < 0 ? args : args.slice(0, cut)).flatMap(files);
  const theirs = cut < 0 ? [] : args.slice(cut + 1).flatMap(files);
  if (!ours.length && !theirs.length) {
    console.error("usage: pe-imports.mjs <ours: file|dir>... [--third-party <file|dir>...]");
    process.exit(2);
  }
  let bad = 0;
  for (const [f, own] of [...ours.map((f) => [f, true]), ...theirs.map((f) => [f, false])]) {
    const names = peImports(readFileSync(f));
    const hits = names.filter((n) => REDIST.test(n) || (own && UCRT.test(n)));
    console.log(`${own ? "ours" : "third-party"} ${f}: ${names.join(", ") || "(no imports)"}`);
    if (hits.length) {
      bad += 1;
      console.error(`::error::${f} needs the VC++ runtime (${hits.join(", ")})${own ? ": link it with +crt-static" : ""}`);
    }
  }
  if (bad) process.exit(1);
  console.log(`${ours.length} of ours with no C runtime imports; ${theirs.length} third-party with no VC++ Redistributable imports`);
}
