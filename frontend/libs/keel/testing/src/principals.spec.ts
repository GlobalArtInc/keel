import { resolve } from 'node:path';
import type { Level } from '@globalart/keel/meta';
import {
  DUMP_LEVELS,
  DUMP_LOGINS,
  KEEL_PRINCIPALS,
  actingAs,
  readDump,
  type DumpLevel,
} from '@globalart/keel/testing';

const DUMP = resolve(__dirname, '../../../../../contracts/dump');

describe('KEEL_PRINCIPALS', () => {
  it('names the four dump levels and equals the principal of each dump', () => {
    expect(Object.keys(KEEL_PRINCIPALS).sort()).toEqual([...DUMP_LEVELS].sort());
    for (const level of DUMP_LEVELS) {
      const dump = readDump(DUMP, level);
      expect(dump.boot.principal ?? null).toEqual(KEEL_PRINCIPALS[level]);
    }
  });

  it('carries the level of its name and the login the dump was taken with', () => {
    const levelled: Exclude<DumpLevel, 'guest'>[] = ['user', 'reseller', 'admin'];
    for (const level of levelled) {
      const principal = KEEL_PRINCIPALS[level];
      expect(principal?.level).toBe<Level>(level);
      expect(principal?.login).toBe(DUMP_LOGINS[level]);
      expect(principal?.actor).toBeNull();
    }
    expect(KEEL_PRINCIPALS.guest).toBeNull();
  });

  it('actingAs sets the actor of a principal through su', () => {
    const alice = KEEL_PRINCIPALS.user;
    const root = KEEL_PRINCIPALS.admin;
    if (alice === null || root === null) {
      throw new Error('the user and admin fixtures are principals');
    }
    expect(actingAs(alice, root)).toEqual({
      ...alice,
      actor: { id: root.id, login: 'root', level: 'admin', via: 'su' },
    });
    expect(alice.actor).toBeNull();
  });
});
