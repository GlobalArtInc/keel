import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import prettier from 'prettier';

const CONTRACTS = resolve(import.meta.dirname, '../../contracts');
const OUT = resolve(import.meta.dirname, '../libs/keel/testing/src/generated/boot.ts');

const SINGLE = 'demo';
const EFFECTIVE_LOGO = '/brand/logo.svg';

const FIXTURES = [
  ['BOOT_NONE', 'fixtures/boot-none.json', (boot) => boot],
  ['BOOT_ONE', 'dump/admin/boot.json', (boot) => narrowed(boot, SINGLE)],
  ['BOOT_TWO', 'fixtures/boot.json', (boot) => boot],
];

function narrowed(boot, id) {
  const product = boot.products.find((entry) => entry.id === id);
  const others = new Set(boot.products.filter((entry) => entry.id !== id).map((entry) => entry.id));
  return {
    ...boot,
    products: [product],
    brand: {
      name: product.brand.name,
      logo: EFFECTIVE_LOGO,
      hue: product.brand.hue,
      chroma: product.brand.chroma,
    },
    features: boot.features.filter((feature) => !others.has(feature.split('.')[0])),
    menu: boot.menu.filter((group) => !others.has(group.product)),
  };
}

function constant(name, file, shape) {
  const payload = shape(JSON.parse(readFileSync(resolve(CONTRACTS, file), 'utf8')));
  return `export const ${name}: Boot = ${JSON.stringify(payload)};\n`;
}

const source = [
  "import type { Boot } from '@globalart/keel/meta';\n",
  ...FIXTURES.map(([name, file, shape]) => constant(name, file, shape)),
].join('\n');

const options = await prettier.resolveConfig(OUT);
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, await prettier.format(source, { ...options, filepath: OUT }));
console.log(`${FIXTURES.length} boot fixtures written to ${OUT}`);
