import { onceOf } from '@globalart/keel/forms';
import type { Descriptor } from '@globalart/keel/meta';

const CREATE: Descriptor = {
  keel: 1,
  name: 'auth.token.create',
  module: 'auth',
  kind: 'form',
  mode: 'write',
  access: { level: 'user' },
  params: { type: 'object' },
  result: {
    type: 'object',
    properties: {
      id: { type: 'string' },
      token: { $ref: '#/$defs/Once' },
      recovery: {
        type: 'object',
        properties: {
          codes: { type: 'array', items: { type: 'string' }, 'x-keel': { once: true } },
        },
      },
      challenge: { anyOf: [{ type: 'string' }, { type: 'null' }], 'x-keel': { once: true } },
    },
    $defs: { Once: { type: 'string', 'x-keel': { once: true } } },
  },
};

describe('once', () => {
  it('lists the once fields of the result that carry a value, through $ref and nested objects', () => {
    expect(
      onceOf(CREATE, {
        id: 'T1',
        token: 'kat_1',
        recovery: { codes: ['a', 'b'] },
        challenge: null,
      }),
    ).toEqual([
      { path: 'token', labelKey: 'auth.token.token', value: 'kat_1' },
      { path: 'recovery.codes', labelKey: 'auth.token.recovery.codes', value: '["a","b"]' },
    ]);
  });

  it('answers nothing for a result without once fields or without an object', () => {
    expect(onceOf(CREATE, { id: 'T1' })).toEqual([]);
    expect(onceOf(CREATE, 'T1')).toEqual([]);
    expect(onceOf({ ...CREATE, result: true }, { token: 'x' })).toEqual([]);
  });
});
