import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed } from '@angular/core/testing';
import {
  KeelBoot,
  KeelCallError,
  KeelClient,
  KeelDescriptors,
  KeelI18n,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { firstValueFrom } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const BOOT = load<Boot>('boot.json');
const LIST = load<Descriptor>('descriptor-dns-zone-list.json');
const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const REFUSED = load<Envelope>('envelope-422.json');
const OK = load<Envelope>('envelope-200.json');

function mock(): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [LIST, EDIT],
    catalogue: { lang: 'en', messages: { dns: { zone: { list: { title: 'Zones' } } } } },
    answers: { 'dns.zone.edit': REFUSED },
  });
}

describe('KeelMockClient', () => {
  it('answers boot, the descriptors and a canned envelope, recording the call', async () => {
    const client = mock();
    TestBed.configureTestingModule({ providers: [{ provide: KeelClient, useValue: client }] });

    const boot = TestBed.inject(KeelBoot);
    expect(await boot.load()).toEqual(BOOT);
    expect(boot.principal()?.login).toBe('res1');
    expect(client.metaVersion()).toBe(BOOT.meta);

    const descriptors = TestBed.inject(KeelDescriptors);
    expect(await descriptors.get('dns.zone.list')).toEqual(LIST);
    expect(await descriptors.list()).toEqual([LIST, EDIT]);
    const missing = descriptors.get('dns.zone.gone');
    await expect(missing).rejects.toBeInstanceOf(KeelCallError);
    await expect(missing).rejects.toMatchObject({ error: { code: 'not_found' } });

    const i18n = TestBed.inject(KeelI18n);
    await i18n.load('en');
    expect(i18n.t('dns.zone.list.title')).toBe('Zones');

    const answered = await firstValueFrom(
      client.call('dns.zone.edit', { id: 'z-1', ttl: 300 }, { idempotencyKey: 'edit-z-1' }),
    );
    expect(answered).toEqual(REFUSED);
    expect(client.lastRequestId()).toBe('req-1a2c');
    expect(client.calls).toEqual([
      {
        func: 'dns.zone.edit',
        params: { id: 'z-1', ttl: 300 },
        options: { idempotencyKey: 'edit-z-1' },
      },
    ]);
    expect(client.callsOf('dns.zone.edit')).toHaveLength(1);
    expect(client.callsOf('dns.zone.list')).toEqual([]);
  });

  it('throws on a function with no canned answer', () => {
    const client = mock();
    expect(() => client.call('dns.zone.delete', { ids: [] })).toThrow(
      'no canned answer for dns.zone.delete',
    );
    expect(client.calls.map((call) => call.func)).toEqual(['dns.zone.delete']);
    expect(() => new KeelMockClient().boot()).toThrow('no boot payload');
  });

  it('a sequence of answers is consumed in order and its last one repeats', async () => {
    const client = mock().answer('dns.zone.list', [OK, REFUSED]);
    const answers: Envelope[] = [];
    for (let attempt = 0; attempt < 3; attempt += 1) {
      answers.push(await firstValueFrom(client.call('dns.zone.list')));
    }
    expect(answers).toEqual([OK, REFUSED, REFUSED]);
    expect(client.calls).toHaveLength(3);
    expect(client.calls[0]).toEqual({ func: 'dns.zone.list', params: {}, options: {} });
  });

  it('changeMeta and expire drive the two events of the client', () => {
    const client = mock();
    const versions: string[] = [];
    let bursts = 0;
    client.metaChanged.subscribe((version) => versions.push(version));
    client.unauthenticated.subscribe(() => {
      bursts += 1;
    });
    client.changeMeta('m-next');
    client.expire();
    expect(versions).toEqual(['m-next']);
    expect(client.metaVersion()).toBe('m-next');
    expect(bursts).toBe(1);
  });
});
