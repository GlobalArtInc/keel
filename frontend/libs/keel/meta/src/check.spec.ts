import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import {
  CHECKS,
  check,
  checkCode,
  checkKey,
  ipFamily,
  isCheck,
  type Check,
} from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');

interface CheckCase {
  check: Check;
  value: string;
  expect: boolean;
}

function cases(): CheckCase[] {
  return JSON.parse(readFileSync(join(FIXTURES, 'check-cases.json'), 'utf8')) as CheckCase[];
}

describe('check', () => {
  it('every case of check-cases.json matches its expected verdict and the twelve presets are all covered', () => {
    const found = cases();
    const covered = new Set(found.map((entry) => entry.check));
    expect([...covered].sort()).toEqual([...CHECKS].sort());
    expect(covered.size).toBe(CHECKS.length);
    for (const entry of found) {
      expect(isCheck(entry.check)).toBe(true);
      const verdict = check(entry.check, entry.value);
      expect({ check: entry.check, value: entry.value, verdict }).toEqual({
        check: entry.check,
        value: entry.value,
        verdict: entry.expect,
      });
    }
  });

  it('a failed check names its error code and key', () => {
    expect(checkCode('ip')).toBe('check.ip');
    expect(checkKey('ip')).toBe('keel.check.ip');
    for (const preset of CHECKS) {
      expect(checkCode(preset)).toBe(`check.${preset}`);
      expect(checkKey(preset)).toBe(`keel.check.${preset}`);
    }
    expect(isCheck('phone')).toBe(false);
    expect(isCheck('IP')).toBe(false);
  });

  it('ip parsing follows the standard library of the kernel, family included', () => {
    expect(ipFamily('10.0.0.1')).toBe(4);
    expect(ipFamily('::ffff:10.0.0.1')).toBe(6);
    expect(ipFamily('1:2:3:4:5:6:1.2.3.4')).toBe(6);
    expect(ipFamily('1:2:3:4:5:6:7::')).toBe(6);
    expect(ipFamily('::1:2:3:4:5:6:7')).toBe(6);
    expect(ipFamily('1:2:1.2.3.4::')).toBeUndefined();
    expect(ipFamily('1:2:3:4:5:6:7:8:9')).toBeUndefined();
    expect(ipFamily('01.1.1.1')).toBeUndefined();
    expect(ipFamily('1.1.1.1000')).toBeUndefined();
    expect(ipFamily('12345::')).toBeUndefined();
    expect(ipFamily('1:2:3:4:5:6:7:8::')).toBeUndefined();
    expect(ipFamily('::1.2.3.4.5')).toBeUndefined();
    expect(ipFamily('1.2.3.4.')).toBeUndefined();
    expect(ipFamily('٣.1.1.1')).toBeUndefined();
    expect(ipFamily('ABCD:ef01::')).toBe(6);
    expect(ipFamily('0000:0000:0000:0000:0000:0000:0000:0001')).toBe(6);
    expect(ipFamily('1::2::3')).toBeUndefined();
    expect(ipFamily(':::')).toBeUndefined();
    expect(ipFamily(':1')).toBeUndefined();
    expect(ipFamily('1:')).toBeUndefined();
  });

  it('int is bounded by the signed 64-bit range in either direction', () => {
    expect(check('int', '9223372036854775807')).toBe(true);
    expect(check('int', '9223372036854775808')).toBe(false);
    expect(check('int', '-9223372036854775808')).toBe(true);
    expect(check('int', '-9223372036854775809')).toBe(false);
    expect(check('int', '-0')).toBe(true);
  });

  it('a non-ascii character fails every preset but path', () => {
    expect(check('domain', 'ünicode.org')).toBe(false);
    expect(check('email', 'ü@a.org')).toBe(false);
    expect(check('name', 'wéb')).toBe(false);
    expect(check('int', '１')).toBe(false);
    expect(check('port', '８０')).toBe(false);
    expect(check('path', '/var/wéb')).toBe(true);
  });
});
