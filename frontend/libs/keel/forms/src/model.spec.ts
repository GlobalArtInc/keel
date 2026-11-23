import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Descriptor } from '@globalart/keel/meta';
import { buildForm } from './model';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');

describe('buildForm in create mode', () => {
  it('a field the screen names takes the screen value unless the given values hold one', () => {
    const form = buildForm(EDIT, { params: { account: 'acc-1', ttl: 300 }, values: { ttl: 60 } });
    expect(form.mode).toBe('create');
    expect(form.field('account')?.control.value).toBe('acc-1');
    expect(form.field('ttl')?.control.value).toBe(60);
  });
});
