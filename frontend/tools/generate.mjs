import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { compile } from 'json-schema-to-typescript';
import prettier from 'prettier';
import ts from 'typescript';

const SCHEMA = resolve(import.meta.dirname, '../../contracts/schema/keel.json');
const OUT = resolve(import.meta.dirname, '../libs/keel/meta/src/generated');
const ROOT = 'KeelSchemaDocument';
const OPEN_OBJECT = 'Record<string, unknown>';
const ANNOTATIONS = new Set([
  'default',
  'deprecated',
  'description',
  'examples',
  'readOnly',
  'title',
  'writeOnly',
]);

function isUnconstrained(node) {
  return Object.keys(node).every((key) => ANNOTATIONS.has(key));
}

function isOpenObject(node) {
  const types = Array.isArray(node.type) ? node.type : [node.type];
  return (
    types.includes('object') &&
    node.properties === undefined &&
    node.patternProperties === undefined &&
    (node.additionalProperties === undefined || node.additionalProperties === true)
  );
}

function tsTypeOfOpenObject(node) {
  const types = Array.isArray(node.type) ? node.type : [node.type];
  return types.map((type) => (type === 'object' ? OPEN_OBJECT : type)).join(' | ');
}

function toDraft07(node) {
  if (Array.isArray(node)) {
    return node.map(toDraft07);
  }
  if (node === null || typeof node !== 'object') {
    return node;
  }
  const out = {};
  for (const [key, value] of Object.entries(node)) {
    out[key] = toDraft07(value);
  }
  if (Array.isArray(out.prefixItems)) {
    if (out.items === false) {
      out.additionalItems = false;
    }
    out.items = out.prefixItems;
    delete out.prefixItems;
  }
  if (isUnconstrained(out)) {
    out.tsType = 'unknown';
  } else if (isOpenObject(out)) {
    out.tsType = tsTypeOfOpenObject(out);
  }
  return out;
}

function withoutRootAndComments(source) {
  const file = ts.createSourceFile(
    'keel.ts',
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const kept = file.statements.filter(
    (statement) =>
      !(
        (ts.isInterfaceDeclaration(statement) || ts.isTypeAliasDeclaration(statement)) &&
        statement.name.text === ROOT
      ),
  );
  const printer = ts.createPrinter({ removeComments: true, newLine: ts.NewLineKind.LineFeed });
  return printer.printFile(ts.factory.updateSourceFile(file, kept));
}

async function format(path, source) {
  const options = await prettier.resolveConfig(path);
  return prettier.format(source, { ...options, filepath: path });
}

const schema = JSON.parse(readFileSync(SCHEMA, 'utf8'));
delete schema.$id;
const generated = await compile(toDraft07(schema), ROOT, {
  additionalProperties: false,
  bannerComment: '',
  format: false,
  unreachableDefinitions: true,
});

mkdirSync(OUT, { recursive: true });
const keel = join(OUT, 'keel.ts');
writeFileSync(keel, await format(keel, withoutRootAndComments(generated)));
const index = join(OUT, 'index.ts');
writeFileSync(index, await format(index, "export * from './keel';\n"));
