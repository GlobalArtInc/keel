import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { Descriptor } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
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

describe('buildForm with a remote select that starts with a value', () => {
  it('looks the label of the initial value up once and shows it', () => {
    const client = new KeelMockClient({
      answers: {
        'keel.account.options': {
          ok: true,
          data: { data: [{ value: 'acme-1', label: 'Acme' }], more: false },
          meta: { requestId: 'req-model' },
        },
      },
    });
    const form = buildForm(EDIT, { values: { account: 'acme-1' }, client });
    expect(client.callsOf('keel.account.options')).toHaveLength(1);
    expect(form.field('account')?.options?.options()).toEqual([{ value: 'acme-1', label: 'Acme' }]);
  });
});
