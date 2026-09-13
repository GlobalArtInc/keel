import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import {
  RECOMPUTE_DEBOUNCE_MS,
  applyAnswer,
  buildForm,
  findingsOf,
  recomputeFields,
  type KeelForm,
  type RecomputeAnswer,
} from '@globalart/keel/forms';
import type { Descriptor, Envelope, FieldError } from '@globalart/keel/meta';
import { KeelMockClient } from '@globalart/keel/testing';
import { throwError } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

const EDIT = load<Descriptor>('descriptor-dns-zone-edit.json');
const RECOMPUTE = 'dns.zone.recompute';
const TTL_ERROR: FieldError = {
  path: 'ttl',
  code: 'dns.zone.error.ttl_short',
  key: 'dns.zone.error.ttl_short',
  params: { minimum: 60 },
};

function answer(data: RecomputeAnswer): Envelope<RecomputeAnswer> {
  return { ok: true, data, meta: { requestId: 'req-recompute' } };
}

function zoneForm(client: KeelMockClient): KeelForm {
  return buildForm(EDIT, {
    client,
    values: { name: 'a.org', kind: 'master', ttl: 300 },
  });
}

describe('recompute', () => {
  afterEach(() => jest.useRealTimers());

  it('a change of ttl calls dns.zone.recompute with { values, changed: "ttl" } once after the debounce, the answered values patch is applied without a second call, and answered errors land on their fields', () => {
    jest.useFakeTimers();
    const client = new KeelMockClient({
      answers: {
        [RECOMPUTE]: answer({ values: { ttl: 3600, name: 'patched.org' }, errors: [TTL_ERROR] }),
      },
    });
    const form = zoneForm(client);
    expect(recomputeFields(form).map((field) => field.name)).toEqual(['ttl']);

    form.group.controls['ttl'].setValue(30);
    form.group.controls['ttl'].setValue(45);
    expect(client.calls).toHaveLength(0);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS - 1);
    expect(client.calls).toHaveLength(0);
    jest.advanceTimersByTime(1);
    expect(client.calls).toHaveLength(1);
    expect(client.calls[0]).toEqual({
      func: RECOMPUTE,
      params: {
        values: { name: 'a.org', kind: 'master', ttl: 45, account: null },
        changed: 'ttl',
      },
      options: {},
    });

    expect(form.group.controls['ttl'].value).toBe(3600);
    expect(form.group.controls['name'].value).toBe('patched.org');
    expect(form.values()['ttl']).toBe(3600);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS * 2);
    expect(client.calls).toHaveLength(1);

    expect(findingsOf(form.group.controls['ttl'])).toEqual([TTL_ERROR]);
    expect(form.group.controls['ttl'].touched).toBe(true);
    expect(form.unmapped()).toEqual([]);

    form.group.controls['name'].setValue('b.org');
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(client.calls).toHaveLength(1);
    form.destroy();
  });

  it('a change after destroy calls nothing', () => {
    jest.useFakeTimers();
    const client = new KeelMockClient({ answers: { [RECOMPUTE]: answer({}) } });
    const form = zoneForm(client);
    form.destroy();
    form.group.controls['ttl'].setValue(1);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(client.calls).toHaveLength(0);
  });

  it('answered options replace the choices of a local select and of a remote one', () => {
    const client = new KeelMockClient({ answers: { [RECOMPUTE]: answer({}) } });
    const form = zoneForm(client);
    const kind = form.field('kind');
    const account = form.field('account');
    if (kind === undefined || account === undefined || account.options === null) {
      throw new Error('the zone form lost kind or account');
    }
    expect(kind.choices()).toBeNull();
    applyAnswer(form, {
      options: {
        kind: [{ value: 'master', label: 'Primary' }],
        account: [{ value: '01J8A0000000000000000000A1', label: 'ACME' }],
        unknown: [{ value: 'x', label: 'X' }],
      },
    });
    expect(kind.choices()).toEqual([{ value: 'master', label: 'Primary' }]);
    expect(account.options.choices()).toEqual([
      { value: '01J8A0000000000000000000A1', label: 'ACME' },
    ]);
    expect(client.calls).toHaveLength(0);
    form.destroy();
  });

  it('a failed recompute answer shows on the banner', () => {
    jest.useFakeTimers();
    const client = new KeelMockClient({
      answers: {
        [RECOMPUTE]: { ok: false, error: { code: 'internal', key: 'keel.error.internal' } },
      },
    });
    const form = zoneForm(client);
    form.group.controls['ttl'].setValue(1);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(form.unmapped()).toEqual([{ path: '', code: 'internal', key: 'keel.error.internal' }]);
    form.destroy();
  });

  it('a transport failure lands on the banner and later changes still recompute', () => {
    jest.useFakeTimers();
    const client = new KeelMockClient({ answers: { [RECOMPUTE]: answer({ values: { ttl: 7 } }) } });
    jest
      .spyOn(client, 'call')
      .mockReturnValueOnce(throwError(() => new Error('connection refused')));
    const form = zoneForm(client);
    form.group.controls['ttl'].setValue(1);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(form.unmapped()).toEqual([
      {
        path: '',
        code: 'unavailable',
        key: 'keel.error.unavailable',
        params: { reason: 'connection refused' },
      },
    ]);
    form.group.controls['ttl'].setValue(2);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(form.group.controls['ttl'].value).toBe(7);
    form.destroy();
  });

  it('a form without a client recomputes nothing', () => {
    jest.useFakeTimers();
    const form = buildForm(EDIT, { values: { kind: 'master' } });
    form.group.controls['ttl'].setValue(1);
    jest.advanceTimersByTime(RECOMPUTE_DEBOUNCE_MS);
    expect(form.client).toBeNull();
    expect(form.group.controls['ttl'].value).toBe(1);
    form.destroy();
  });
});
