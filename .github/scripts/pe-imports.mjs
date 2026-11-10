// Checks that Windows binaries we ship don't need the VC++ runtime;
// dependency-free (no dumpbin / Visual Studio needed).
//
//   node pe-imports.mjs <file.exe|.dll|dir>...
//
// Reads each PE file's import and delay-import tables and fails if one names
// vcruntime*.dll, msvcp*.dll or api-ms-win-crt-*.dll: those come with the VC++
// Redistributable, which a clean Windows install does not have (v0.3.0:
// "VCRUNTIME140.dll was not found"). A directory is searched for .exe/.dll.
// Prints every file's imports so the build log shows what was checked.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const FORBIDDEN = /^(vcruntime\d*(_\d+)?|msvcp\d+(_\w+)?|api-ms-win-crt-[\w-]+)\.dll$/i;

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
  const list = process.argv.slice(2).flatMap(files);
  if (!list.length) {
    console.error("usage: pe-imports.mjs <file.exe|.dll|dir>...");
    process.exit(2);
  }
  let bad = 0;
  for (const f of list) {
    const names = peImports(readFileSync(f));
    const hits = names.filter((n) => FORBIDDEN.test(n));
    console.log(`${f}: ${names.join(", ") || "(no imports)"}`);
    if (hits.length) {
      bad += 1;
      console.error(`::error::${f} needs the VC++ runtime (${hits.join(", ")}): link it with +crt-static`);
    }
  }
  if (bad) process.exit(1);
  console.log(`${list.length} file(s): no VC++ runtime imports`);
}
