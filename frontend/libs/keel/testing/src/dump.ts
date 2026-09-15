import type { Catalogue } from '@globalart/keel/client';
import type { Boot, Descriptor } from '@globalart/keel/meta';
import type { KeelMockFixtures } from './mock-client';

export const DUMP_LEVELS = ['guest', 'user', 'reseller', 'admin'] as const;
export type DumpLevel = (typeof DUMP_LEVELS)[number];

export const BOOT_FILE = 'boot.json';
export const FUNCTIONS_DIR = 'functions';
export const I18N_DIR = 'i18n';
export const DUMP_LANG = 'en';

const JSON_SUFFIX = '.json';

export interface KeelDump {
  level: DumpLevel;
  boot: Boot;
  descriptors: Descriptor[];
  catalogues: Record<string, Catalogue>;
}

export type DumpFiles = ReadonlyMap<string, unknown>;

interface FileSystem {
  readdirSync(path: string, options: { recursive: true; encoding: 'utf8' }): string[];
  readFileSync(path: string, encoding: 'utf8'): string;
}

type Host = { process?: { getBuiltinModule?: (id: string) => unknown } };

export function isDumpLevel(value: string): value is DumpLevel {
  return (DUMP_LEVELS as readonly string[]).includes(value);
}

function nameOf(path: string, dir: string): string | null {
  const prefix = `${dir}/`;
  if (
    !path.startsWith(prefix) ||
    !path.endsWith(JSON_SUFFIX) ||
    path.indexOf('/', prefix.length) >= 0
  ) {
    return null;
  }
  return path.slice(prefix.length, -JSON_SUFFIX.length);
}

export function dumpOf(level: DumpLevel, files: DumpFiles): KeelDump {
  const boot = files.get(BOOT_FILE);
  if (boot === undefined) {
    throw new Error(`the dump of ${level} has no ${BOOT_FILE}`);
  }
  const descriptors: Descriptor[] = [];
  const catalogues: Record<string, Catalogue> = {};
  for (const [path, content] of files) {
    if (nameOf(path, FUNCTIONS_DIR) !== null) {
      descriptors.push(content as Descriptor);
    }
    const lang = nameOf(path, I18N_DIR);
    if (lang !== null) {
      catalogues[lang] = content as Catalogue;
    }
  }
  descriptors.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  return { level, boot: boot as Boot, descriptors, catalogues };
}

export function dumpsOf(files: DumpFiles): KeelDump[] {
  const byLevel = new Map<DumpLevel, Map<string, unknown>>();
  for (const [path, content] of files) {
    const slash = path.indexOf('/');
    const level = slash < 0 ? '' : path.slice(0, slash);
    if (!isDumpLevel(level)) {
      continue;
    }
    let own = byLevel.get(level);
    if (own === undefined) {
      own = new Map();
      byLevel.set(level, own);
    }
    own.set(path.slice(slash + 1), content);
  }
  return DUMP_LEVELS.filter((level) => byLevel.has(level)).map((level) =>
    dumpOf(level, byLevel.get(level) ?? new Map()),
  );
}

function fileSystem(): FileSystem {
  const fs = (globalThis as Host).process?.getBuiltinModule?.('node:fs');
  if (fs === undefined) {
    throw new Error('a dump directory can only be read under node');
  }
  return fs as FileSystem;
}

export function readDump(dir: string, level: DumpLevel): KeelDump {
  const fs = fileSystem();
  const root = `${dir}/${level}`;
  const files = new Map<string, unknown>();
  for (const path of fs.readdirSync(root, { recursive: true, encoding: 'utf8' })) {
    if (path.endsWith(JSON_SUFFIX)) {
      files.set(path, JSON.parse(fs.readFileSync(`${root}/${path}`, 'utf8')));
    }
  }
  return dumpOf(level, files);
}

export function catalogueOf(dump: KeelDump): Catalogue {
  const lang = dump.boot.principal?.lang ?? DUMP_LANG;
  return dump.catalogues[lang] ?? dump.catalogues[DUMP_LANG] ?? { lang, messages: {} };
}

export function fixturesOf(dump: KeelDump): KeelMockFixtures {
  return { boot: dump.boot, descriptors: dump.descriptors, catalogue: catalogueOf(dump) };
}
