import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { extname, join, relative, resolve } from 'node:path';
import { brotliCompressSync, constants } from 'node:zlib';

const COMPRESSIBLE = new Set(['.js', '.mjs', '.css', '.html', '.svg', '.json', '.txt', '.map']);
const TWIN = '.br';

const root = resolve(process.argv[2] ?? 'dist/shell');

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
}

function compress(path) {
  const source = readFileSync(path);
  return brotliCompressSync(source, {
    params: {
      [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
      [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
      [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
    },
  });
}

if (!statSync(root, { throwIfNoEntry: false })?.isDirectory()) {
  console.error(`${root} is not a directory; build the shell first`);
  process.exit(1);
}

let written = 0;
let skipped = 0;
for (const path of walk(root)) {
  if (path.endsWith(TWIN) || !COMPRESSIBLE.has(extname(path))) {
    continue;
  }
  const compressed = compress(path);
  if (compressed.length >= statSync(path).size) {
    skipped += 1;
    continue;
  }
  writeFileSync(`${path}${TWIN}`, compressed);
  written += 1;
  console.log(`${relative(root, path)}${TWIN}: ${compressed.length} bytes`);
}
console.log(`${written} brotli twins written under ${root}, ${skipped} files left uncompressed`);
