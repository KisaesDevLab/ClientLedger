// Bundle the compiled MyBooks engine (packages/api/dist) into one CommonJS file
// plus a SEA config. Usage: node bundle.mjs <engineDir> <outDir> <platformTag> <engineCommit>
//
// The engine is never edited. Things it expects on disk (migrations, knowledge
// files, fonts, the bcrypt native addon) are embedded as SEA assets and
// extracted on first start into a per-bundle runtime directory; the bundle
// banner points `import.meta.url` into that directory so the engine's own
// relative lookups keep working.
import { build } from 'esbuild';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, resolve, sep } from 'node:path';

const [engineDir, outDir] = process.argv.slice(2, 4).map((p) => resolve(p));
const tag = process.argv[4];
// Source commit of the engine ('<sha>' or '<sha>-dirty'), reported on /health.
const engineCommit = process.argv[5];
if (!engineDir || !outDir || !tag || !/^[0-9a-f]{7,64}(-dirty)?$/.test(engineCommit ?? '')) {
  console.error('usage: node bundle.mjs <engineDir> <outDir> <platformTag> <engineCommit[-dirty]>');
  process.exit(2);
}
const api = join(engineDir, 'packages', 'api');
const nm = join(engineDir, 'node_modules');
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

// Files shipped as assets: [assetKey (path under runtime dir), absolute source].
const assets = [];
const addDir = (srcDir, keyPrefix) => {
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else assets.push([`${keyPrefix}/${relative(srcDir, p).split(sep).join('/')}`, p]);
    }
  };
  walk(srcDir);
};
addDir(join(api, 'src', 'db', 'migrations'), 'db/migrations');
addDir(join(api, 'src', 'knowledge'), 'knowledge');
addDir(join(api, 'assets', 'fs-fonts'), 'assets/fs-fonts');

// bcrypt: ship only the prebuild for this platform.
const bcryptDir = join(nm, 'bcrypt', 'prebuilds', tag);
const bcryptFile = existsSync(bcryptDir)
  ? readdirSync(bcryptDir).filter((f) => f.endsWith('.node')).sort((a, b) => (a.includes('glibc') ? -1 : 1) - (b.includes('glibc') ? -1 : 1))[0]
  : undefined;
if (!bcryptFile) throw new Error(`no bcrypt prebuild for ${tag} in ${bcryptDir}`);
assets.push(['native/bcrypt.node', join(bcryptDir, bcryptFile)]);

const shimDir = join(outDir, '_shims');
mkdirSync(shimDir);
writeFileSync(join(shimDir, 'gyp.cjs'), `
module.exports = function nodeGypBuild() {
  const m = { exports: {} };
  process.dlopen(m, require('node:path').join(globalThis.__cl_rt, 'native', 'bcrypt.node'));
  return m.exports;
};
`);
// Puppeteer needs a downloaded Chromium and is only used for HTML-to-PDF paths
// that ClientLedger replaces with cl-print; stub it so imports resolve.
writeFileSync(join(shimDir, 'puppeteer.cjs'), `
const unavailable = () => { throw new Error('PDF rendering through Chromium is not available in ClientLedger'); };
const p = { launch: unavailable, executablePath: unavailable };
module.exports = p; module.exports.default = p; module.exports.__esModule = true;
`);

// pdf.js (pdfjs-dist, Apache-2.0) reads the text layer of PDFs (bill and
// statement reading). In Node it runs its worker in-process and loads it with
// `import("./pdf.worker.mjs")` next to pdf.mjs, a file the single executable
// does not have. pdf.js uses `globalThis.pdfjsWorker` first when it is set,
// so the entry below bundles the worker and sets that global before the
// engine starts. Scanned PDFs still need poppler's `pdftoppm` (GPL), which
// ClientLedger does not ship; the app says so instead of failing.
const pdfWorker = join(nm, 'pdfjs-dist', 'legacy', 'build', 'pdf.worker.mjs');
if (!existsSync(pdfWorker)) throw new Error(`pdf.js worker missing: ${pdfWorker}`);
const fwd = (p) => JSON.stringify(p.split(sep).join('/'));
writeFileSync(join(shimDir, 'pdfjs-worker.mjs'), `
import * as worker from ${fwd(pdfWorker)};
globalThis.pdfjsWorker = worker;
`);
// ESM evaluates imports in order: the worker global is set before bootstrap.
const entry = join(shimDir, 'entry.mjs');
writeFileSync(entry, `
import './pdfjs-worker.mjs';
import ${fwd(join(api, 'dist', 'bootstrap.js'))};
`);

const banner = `
process.env['ENGINE_COMMIT'] = ${JSON.stringify(engineCommit)};
var __cl_boot = (function () {
  const fs = require('node:fs'), path = require('node:path'), os = require('node:os'), url = require('node:url');
  const sea = require('node:sea');
  const list = JSON.parse(sea.getAsset('assets.json', 'utf8'));
  const id = sea.getAsset('bundle.id', 'utf8').trim();
  const base = process.env.CL_ENGINE_RUNTIME_DIR || path.join(os.tmpdir(), 'clientledger-engine');
  const rt = path.join(base, id);
  if (!fs.existsSync(path.join(rt, '.complete'))) {
    const part = rt + '.partial-' + process.pid;
    fs.rmSync(part, { recursive: true, force: true });
    for (const key of list) {
      const dest = path.join(part, key);
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.writeFileSync(dest, Buffer.from(sea.getRawAsset(key)));
    }
    fs.mkdirSync(path.join(part, 'dist'), { recursive: true });
    fs.writeFileSync(path.join(part, '.complete'), id);
    try { fs.renameSync(part, rt); } catch { fs.rmSync(part, { recursive: true, force: true }); }
  }
  globalThis.__cl_rt = rt;
  process.env.FS_FONT_DIR = process.env.FS_FONT_DIR || path.join(rt, 'assets', 'fs-fonts');
  return url.pathToFileURL(path.join(rt, 'dist', 'bundle.cjs')).href;
})();
`;

const noNative = {
  name: 'cl-shims',
  setup(b) {
    b.onResolve({ filter: /^node-gyp-build$/ }, () => ({ path: join(shimDir, 'gyp.cjs') }));
    b.onResolve({ filter: /^puppeteer(-core)?$/ }, () => ({ path: join(shimDir, 'puppeteer.cjs') }));
    // Optional native accelerators with pure-JS fallbacks.
    b.onResolve({ filter: /^(msgpackr-extract|cpu-features|@napi-rs\/canvas|canvas|bufferutil|utf-8-validate|pg-native|sqlite3|better-sqlite3)$/ }, (a) => ({ path: a.path, external: true }));
    b.onResolve({ filter: /\.node$/ }, (a) => ({ path: a.path, external: true }));
  },
};

const bundlePath = join(outDir, 'bundle.cjs');
const result = await build({
  entryPoints: [entry],
  outfile: bundlePath,
  bundle: true,
  platform: 'node',
  target: 'node24',
  format: 'cjs',
  banner: { js: banner },
  define: { 'import.meta.url': '__cl_boot' },
  plugins: [noNative],
  logLevel: 'warning',
  metafile: true,
  mainFields: ['module', 'main'],
  conditions: ['node'],
});
writeFileSync(join(outDir, 'meta.json'), JSON.stringify(result.metafile));

const bundleSha = createHash('sha256').update(readFileSync(bundlePath)).digest('hex');
const id = bundleSha.slice(0, 16);
writeFileSync(join(outDir, 'bundle.id'), id);
writeFileSync(join(outDir, 'assets.json'), JSON.stringify(assets.map(([k]) => k)));

const seaAssets = { 'assets.json': join(outDir, 'assets.json'), 'bundle.id': join(outDir, 'bundle.id') };
for (const [k, src] of assets) seaAssets[k] = src;
writeFileSync(
  join(outDir, 'sea-config.json'),
  JSON.stringify({
    main: bundlePath,
    output: join(outDir, 'sea-prep.blob'),
    disableExperimentalSEAWarning: true,
    useCodeCache: false,
    assets: seaAssets,
  }),
);
writeFileSync(join(outDir, 'bundle.sha256'), bundleSha);
console.log(`bundle ${bundleSha} (${statSync(bundlePath).size} bytes, ${assets.length} assets, id ${id})`);
