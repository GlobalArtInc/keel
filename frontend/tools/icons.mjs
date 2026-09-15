import '@angular/compiler';
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { PHX_ICON_NAMES } from '@globalart/phoenix-ui';

const OUT = resolve(import.meta.dirname, '../../contracts/icons.json');

const names = [...new Set(PHX_ICON_NAMES)].sort();
if (names.length === 0) {
  console.error('@globalart/phoenix-ui exports no icon names');
  process.exit(1);
}

writeFileSync(OUT, `${JSON.stringify(names, null, 2)}\n`);
console.log(`${names.length} icon names from @globalart/phoenix-ui written to ${OUT}`);
