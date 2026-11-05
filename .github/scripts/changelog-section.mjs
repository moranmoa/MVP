// Prints one version's section of CHANGELOG.md (without its heading), for
// the release notes. Fails if the version has no section or it is empty.
// No dependencies.
//
//   node changelog-section.mjs <X.Y.Z> [CHANGELOG.md]
import { readFileSync } from "node:fs";

const [version, file = "CHANGELOG.md"] = process.argv.slice(2);
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("usage: changelog-section.mjs <X.Y.Z> [CHANGELOG.md]");
  process.exit(2);
}
const lines = readFileSync(file, "utf8").replace(/\r\n/g, "\n").split("\n");
const heading = new RegExp(`^## \\[${version.replace(/\./g, "\\.")}\\]( |$)`);
const start = lines.findIndex((l) => heading.test(l));
if (start < 0) {
  console.error(`::error::${file} has no section "## [${version}]": write one before tagging v${version}`);
  process.exit(1);
}
let end = lines.findIndex((l, i) => i > start && /^## \[/.test(l));
if (end < 0) end = lines.length;
const body = lines.slice(start + 1, end).join("\n").trim();
if (!body) {
  console.error(`::error::the "## [${version}]" section of ${file} is empty`);
  process.exit(1);
}
process.stdout.write(body + "\n");
