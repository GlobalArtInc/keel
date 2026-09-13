import type { Check } from './generated';

export const CHECKS: readonly Check[] = [
  'domain',
  'ip',
  'cidr',
  'email',
  'int',
  'name',
  'port',
  'path',
];

export function isCheck(value: unknown): value is Check {
  return typeof value === 'string' && (CHECKS as readonly string[]).includes(value);
}

export function checkCode(preset: Check): string {
  return `check.${preset}`;
}

export function checkKey(preset: Check): string {
  return `keel.check.${preset}`;
}

export function check(preset: Check, value: string): boolean {
  switch (preset) {
    case 'domain':
      return isDomain(value);
    case 'ip':
      return ipFamily(value) !== undefined;
    case 'cidr':
      return isCidr(value);
    case 'email':
      return isEmail(value);
    case 'int':
      return isInt(value);
    case 'name':
      return isName(value);
    case 'port':
      return isPort(value);
    case 'path':
      return isPath(value);
  }
}

export type IpFamily = 4 | 6;

export function ipFamily(value: string): IpFamily | undefined {
  const parser = new IpParser(value);
  const family = parser.readIpAddr();
  return parser.done ? family : undefined;
}

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const LOCAL_PART_SYMBOLS = ".!#$%&'*+/=?^_`{|}~-";

function isDomain(value: string): boolean {
  if (value.length === 0 || value.length > 253) {
    return false;
  }
  const labels = value.split('.');
  const top = labels[labels.length - 1] ?? '';
  return labels.length >= 2 && labels.every(isLabel) && [...top].some(isAsciiAlphabetic);
}

function isLabel(label: string): boolean {
  return (
    label.length > 0 &&
    label.length <= 63 &&
    [...label].every((char) => isAsciiAlphanumeric(char) || char === '-') &&
    !label.startsWith('-') &&
    !label.endsWith('-')
  );
}

function isCidr(value: string): boolean {
  const slash = value.indexOf('/');
  if (slash < 0) {
    return false;
  }
  const address = value.slice(0, slash);
  const prefix = value.slice(slash + 1);
  if (!isCanonicalDigits(prefix) || prefix.length > 3) {
    return false;
  }
  const bits = Number(prefix);
  switch (ipFamily(address)) {
    case 4:
      return bits <= 32;
    case 6:
      return bits <= 128;
    default:
      return false;
  }
}

function isEmail(value: string): boolean {
  if (value.length > 254) {
    return false;
  }
  const at = value.indexOf('@');
  if (at < 0) {
    return false;
  }
  const local = value.slice(0, at);
  const domain = value.slice(at + 1);
  return (
    local.length <= 64 &&
    local.length > 0 &&
    !local.startsWith('.') &&
    !local.endsWith('.') &&
    !local.includes('..') &&
    [...local].every(isLocalPartChar) &&
    isDomain(domain)
  );
}

function isLocalPartChar(char: string): boolean {
  return isAsciiAlphanumeric(char) || LOCAL_PART_SYMBOLS.includes(char);
}

function isInt(value: string): boolean {
  const digits = value.startsWith('-') ? value.slice(1) : value;
  if (!isCanonicalDigits(digits)) {
    return false;
  }
  const parsed = BigInt(value);
  return parsed >= I64_MIN && parsed <= I64_MAX;
}

function isCanonicalDigits(text: string): boolean {
  return (
    text.length > 0 && [...text].every(isAsciiDigit) && (text.length === 1 || !text.startsWith('0'))
  );
}

function isName(value: string): boolean {
  const [first, ...rest] = value;
  return (
    first !== undefined &&
    isAsciiAlphabetic(first) &&
    value.length <= 64 &&
    rest.every((char) => isAsciiAlphanumeric(char) || char === '_' || char === '-')
  );
}

function isPort(value: string): boolean {
  return isCanonicalDigits(value) && value !== '0' && value.length <= 5 && Number(value) <= 65535;
}

function isPath(value: string): boolean {
  if (value === '/') {
    return true;
  }
  if (!value.startsWith('/')) {
    return false;
  }
  const rest = value.slice(1);
  return (
    ![...value].some((char) => {
      const code = char.charCodeAt(0);
      return code < 0x20 || code === 0x7f;
    }) &&
    rest.split('/').every((segment) => segment.length > 0 && segment !== '.' && segment !== '..')
  );
}

function isAsciiDigit(char: string): boolean {
  return char >= '0' && char <= '9';
}

function isAsciiAlphabetic(char: string): boolean {
  return (char >= 'a' && char <= 'z') || (char >= 'A' && char <= 'Z');
}

function isAsciiAlphanumeric(char: string): boolean {
  return isAsciiDigit(char) || isAsciiAlphabetic(char);
}

function toDigit(char: string, radix: 10 | 16): number | undefined {
  if (isAsciiDigit(char)) {
    return char.charCodeAt(0) - 48;
  }
  if (radix === 16) {
    if (char >= 'a' && char <= 'f') {
      return char.charCodeAt(0) - 87;
    }
    if (char >= 'A' && char <= 'F') {
      return char.charCodeAt(0) - 55;
    }
  }
  return undefined;
}

class IpParser {
  private position = 0;

  constructor(private readonly text: string) {}

  get done(): boolean {
    return this.position === this.text.length;
  }

  readIpAddr(): IpFamily | undefined {
    if (this.readIpv4Addr()) {
      return 4;
    }
    return this.readIpv6Addr() ? 6 : undefined;
  }

  private readAtomically<T>(inner: () => T | undefined): T | undefined {
    const start = this.position;
    const result = inner();
    if (result === undefined) {
      this.position = start;
    }
    return result;
  }

  private peekChar(): string | undefined {
    return this.text[this.position];
  }

  private readChar(): string | undefined {
    const char = this.text[this.position];
    if (char !== undefined) {
      this.position += 1;
    }
    return char;
  }

  private readGivenChar(expected: string): boolean {
    return this.readAtomically(() => (this.readChar() === expected ? true : undefined)) === true;
  }

  private readSeparator<T>(separator: string, index: number, inner: () => T | undefined) {
    return this.readAtomically(() => {
      if (index > 0 && !this.readGivenChar(separator)) {
        return undefined;
      }
      return inner();
    });
  }

  private readNumber(
    radix: 10 | 16,
    maxDigits: number,
    allowZeroPrefix: boolean,
    max: number,
  ): number | undefined {
    return this.readAtomically(() => {
      let digitCount = 0;
      const hasLeadingZero = this.peekChar() === '0';
      let result = 0;
      for (;;) {
        const digit = this.readAtomically(() => {
          const char = this.readChar();
          return char === undefined ? undefined : toDigit(char, radix);
        });
        if (digit === undefined) {
          break;
        }
        result = result * radix + digit;
        digitCount += 1;
        if (digitCount > maxDigits) {
          return undefined;
        }
      }
      if (result > max || digitCount === 0) {
        return undefined;
      }
      if (!allowZeroPrefix && hasLeadingZero && digitCount > 1) {
        return undefined;
      }
      return result;
    });
  }

  private readIpv4Addr(): boolean {
    return (
      this.readAtomically(() => {
        for (let index = 0; index < 4; index += 1) {
          const octet = this.readSeparator('.', index, () => this.readNumber(10, 3, false, 255));
          if (octet === undefined) {
            return undefined;
          }
        }
        return true;
      }) === true
    );
  }

  private readGroups(limit: number): [number, boolean] {
    for (let index = 0; index < limit; index += 1) {
      if (index < limit - 1) {
        const ipv4 = this.readSeparator(':', index, () => (this.readIpv4Addr() ? true : undefined));
        if (ipv4 === true) {
          return [index + 2, true];
        }
      }
      const group = this.readSeparator(':', index, () => this.readNumber(16, 4, true, 0xffff));
      if (group === undefined) {
        return [index, false];
      }
    }
    return [limit, false];
  }

  private readIpv6Addr(): boolean {
    return (
      this.readAtomically(() => {
        const [headSize, headIpv4] = this.readGroups(8);
        if (headSize === 8) {
          return true;
        }
        if (headIpv4) {
          return undefined;
        }
        if (!this.readGivenChar(':') || !this.readGivenChar(':')) {
          return undefined;
        }
        this.readGroups(8 - (headSize + 1));
        return true;
      }) === true
    );
  }
}
