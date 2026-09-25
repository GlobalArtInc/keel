import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const root = resolve(process.argv[2] ?? 'dist/shell');
const products = resolve(process.argv[3] ?? 'products');

if (!statSync(root, { throwIfNoEntry: false })?.isDirectory()) {
  console.error(`${root} is not a directory; build the shell first`);
  process.exit(1);
}

const ids = readdirSync(products, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();
const scripts = readdirSync(root).filter((name) => name.endsWith('.js'));
const initial = scripts
  .filter((name) => /^(main|polyfills)-[A-Z0-9]+\.js$/.test(name))
  .map((name) => readFileSync(join(root, name), 'utf8'));

const problems = [];
for (const id of ids) {
  const pattern = new RegExp(`^${id}-[A-Z0-9]+\\.js$`);
  const chunks = scripts.filter((name) => pattern.test(name));
  if (chunks.length !== 1) {
    problems.push(`the product ${id} has ${chunks.length} chunks named ${id}-<hash>.js, not one`);
    continue;
  }
  const [chunk] = chunks;
  const eager = initial.some(
    (source) => source.includes(`from"./${chunk}"`) || source.includes(`import"./${chunk}"`),
  );
  const lazy = initial.some((source) => source.includes(`import("./${chunk}")`));
  if (eager || !lazy) {
    problems.push(`the chunk ${chunk} of ${id} is not loaded only through a dynamic import`);
    continue;
  }
  console.log(`${id}: ${chunk}, ${statSync(join(root, chunk)).size} bytes, loaded on demand`);
}

for (const problem of problems) {
  console.error(problem);
}
if (problems.length > 0) {
  process.exit(1);
}
console.log(`${ids.length} products, one lazily loaded chunk each, under ${root}`);
