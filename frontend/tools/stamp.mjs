import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const REGISTRY = 'https://astrum.globalart.hu/repository/npm-hosted/';
const ENTRY_POINTS = [
  'meta',
  'client',
  'forms',
  'renderer',
  'shell',
  'testing',
  'testing/playwright',
];
const PACKAGE = resolve(import.meta.dirname, '../dist/keel/package.json');
const WORKSPACE = resolve(import.meta.dirname, '../../Cargo.toml');

function workspaceVersion() {
  const manifest = readFileSync(WORKSPACE, 'utf8');
  const section = manifest.split(/^\[/m).find((part) => part.startsWith('workspace.package]'));
  const version = section?.match(/^version = "([^"]+)"$/m)?.[1];
  if (version === undefined) {
    throw new Error(`${WORKSPACE} carries no [workspace.package] version`);
  }
  return version;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

const requested = process.argv[2];
const binary = workspaceVersion();
if (requested !== undefined && requested !== binary) {
  fail(
    `the version ${requested} is not the binary's ${binary}; the library follows the binary until SDK 1.0`,
  );
}

let pkg;
try {
  pkg = JSON.parse(readFileSync(PACKAGE, 'utf8'));
} catch (error) {
  fail(`${PACKAGE} is not readable (${error.message}); run ng build keel first`);
}

const missing = ENTRY_POINTS.filter((entry) => pkg.exports?.[`./${entry}`] === undefined);
if (pkg.exports?.['.'] === undefined || missing.length > 0) {
  fail(`${PACKAGE} does not export ${missing.length > 0 ? missing.join(', ') : '.'}`);
}

pkg.version = binary;
pkg.publishConfig = { registry: REGISTRY };
delete pkg.scripts;
writeFileSync(PACKAGE, `${JSON.stringify(pkg, null, 2)}\n`);
console.log(
  `${pkg.name}@${pkg.version} stamped for ${REGISTRY} exporting ${ENTRY_POINTS.join(', ')}`,
);
