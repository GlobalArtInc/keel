import { dumpsOf, type KeelDump } from '@globalart/keel/testing';

declare const require: {
  context(
    directory: string,
    recursive: boolean,
    pattern: RegExp,
  ): { keys(): string[]; (key: string): unknown };
};

const DUMP = require.context('../../contracts/dump', true, /\.json$/);

export function bundledDumps(): KeelDump[] {
  const files = new Map<string, unknown>();
  for (const key of DUMP.keys()) {
    files.set(key.replace(/^\.\//, ''), DUMP(key));
  }
  return dumpsOf(files);
}
