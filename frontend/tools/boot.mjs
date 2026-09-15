import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import prettier from 'prettier';

const CONTRACTS = resolve(import.meta.dirname, '../../contracts');
const OUT = resolve(import.meta.dirname, '../libs/keel/testing/src/generated/boot.ts');

const FIXTURES = [
  ['BOOT_NONE', 'fixtures/boot-none.json'],
  ['BOOT_ONE', 'dump/admin/boot.json'],
  ['BOOT_TWO', 'fixtures/boot.json'],
];

function constant(name, file) {
  const payload = JSON.parse(readFileSync(resolve(CONTRACTS, file), 'utf8'));
  return `export const ${name}: Boot = ${JSON.stringify(payload)};\n`;
}

const source = [
  "import type { Boot } from '@globalart/keel/meta';\n",
  ...FIXTURES.map(([name, file]) => constant(name, file)),
].join('\n');

const options = await prettier.resolveConfig(OUT);
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, await prettier.format(source, { ...options, filepath: OUT }));
console.log(`${FIXTURES.length} boot fixtures written to ${OUT}`);
