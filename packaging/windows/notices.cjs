// Builds the complete THIRD-PARTY-NOTICES.txt shipped next to jbd.exe:
//   node packaging/windows/notices.cjs <out file>
//
// = assets/jbd/THIRD-PARTY-NOTICES.txt (FilmCraft, fonts; also built into the app)
// + every Rust library jbd.exe is built from: name, version, licence, and the licence files the
//   library ships (LICENSE*, COPYING*, NOTICE*). Identical texts are printed once, followed by the
//   libraries that use them, so the file stays readable.
const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

const root = path.resolve(__dirname, '..', '..');
const out = process.argv[2] || path.join(root, 'target', 'THIRD-PARTY-NOTICES.txt');
const meta = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--locked'], { cwd: root, maxBuffer: 1 << 28 }).toString());

const byId = new Map(meta.packages.map((p) => [p.id, p]));
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
const workspace = new Set(meta.workspace_members);
const app = meta.packages.find((p) => p.name === 'filmcraft' && workspace.has(p.id));
if (!app) throw new Error('the app package (filmcraft) was not found');

// everything the app links: normal (not dev) dependencies, transitively, any target
const seen = new Set();
const stack = [app.id];
while (stack.length) {
  const id = stack.pop();
  if (seen.has(id)) continue;
  seen.add(id);
  for (const d of nodes.get(id)?.deps ?? []) {
    if (d.dep_kinds.some((k) => k.kind === null || k.kind === 'build')) stack.push(d.pkg);
  }
}
const libs = [...seen].filter((id) => !workspace.has(id)).map((id) => byId.get(id)).filter(Boolean);
libs.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));

const texts = new Map(); // hash -> { text, users: [] }
const lines = [];
for (const p of libs) {
  const dir = path.dirname(p.manifest_path);
  let files = [];
  try {
    files = fs.readdirSync(dir).filter((f) => /^(licen[cs]e|copying|notice)/i.test(f) && fs.statSync(path.join(dir, f)).isFile());
  } catch {}
  if (p.license_file) files.push(p.license_file);
  const keys = [];
  for (const f of [...new Set(files)]) {
    const text = fs.readFileSync(path.join(dir, f), 'utf8').replace(/\r\n/g, '\n').trim();
    const h = crypto.createHash('sha256').update(text).digest('hex').slice(0, 12);
    if (!texts.has(h)) texts.set(h, { text, users: [] });
    texts.get(h).users.push(`${p.name} ${p.version}`);
    keys.push(`[${h}]`);
  }
  lines.push(`${p.name} ${p.version}  -  ${p.license || 'see licence file'}${p.repository ? `  -  ${p.repository}` : ''}${keys.length ? `  ${keys.join(' ')}` : ''}`);
}

const base = fs.readFileSync(path.join(root, 'assets', 'jbd', 'THIRD-PARTY-NOTICES.txt'), 'utf8').replace(/\r\n/g, '\n');
const cut = base.indexOf('The full list of Rust libraries');
let doc = (cut > 0 ? base.slice(0, cut) : base).trimEnd() + '\n\n';
doc += `JBD is built from these ${libs.length} Rust libraries (name, version, licence, source, and the\n`;
doc += 'licence texts below that each one ships, by [id]):\n\n' + lines.join('\n') + '\n\n';
for (const [h, { text, users }] of texts) {
  doc += `===============================================================================\n[${h}] used by: ${users.join(', ')}\n===============================================================================\n\n${text}\n\n`;
}
fs.mkdirSync(path.dirname(out), { recursive: true });
fs.writeFileSync(out, doc.replace(/\n/g, '\r\n'));
console.log(`${out}: ${libs.length} libraries, ${texts.size} distinct licence texts, ${(doc.length / 1024).toFixed(0)} KB`);
