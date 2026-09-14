import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { PHX_THEME_BOOT_SCRIPT } from '@globalart/phoenix-ui';

const TOOL = resolve(__dirname, '../../../tools/theme-boot.mjs');
const INDEX = readFileSync(resolve(__dirname, 'index.html'), 'utf8');

function run(dir: string): string {
  return execFileSync(process.execPath, [TOOL, dir], { encoding: 'utf8', stdio: 'pipe' });
}

describe('theme-boot.js', () => {
  it('is written into the shell build from the phoenix-ui constant', () => {
    const dir = mkdtempSync(join(tmpdir(), 'keel-theme-boot-'));
    try {
      expect(run(dir)).toContain('theme-boot.js');
      expect(readFileSync(join(dir, 'theme-boot.js'), 'utf8')).toBe(`${PHX_THEME_BOOT_SCRIPT}\n`);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('refuses a directory the shell build has not written', () => {
    expect(() => run(join(tmpdir(), 'keel-no-shell'))).toThrow('build the shell first');
  });

  it('is loaded by the index before the application bundle', () => {
    expect(INDEX).toContain('<script src="theme-boot.js"></script>');
    expect(INDEX.indexOf('theme-boot.js')).toBeLessThan(INDEX.indexOf('<body'));
  });
});
