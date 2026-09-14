import '@angular/compiler';
import { statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { PHX_THEME_BOOT_SCRIPT } from '@globalart/phoenix-ui';

const FILE = 'theme-boot.js';

const root = resolve(process.argv[2] ?? 'dist/shell');

if (!statSync(root, { throwIfNoEntry: false })?.isDirectory()) {
  console.error(`${root} is not a directory; build the shell first`);
  process.exit(1);
}

const target = resolve(root, FILE);
writeFileSync(target, `${PHX_THEME_BOOT_SCRIPT}\n`);
console.log(`${FILE}: ${PHX_THEME_BOOT_SCRIPT.length} bytes from @globalart/phoenix-ui`);
