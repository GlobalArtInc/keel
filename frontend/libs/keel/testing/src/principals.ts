import type { BootPrincipal, Level } from '@globalart/keel/meta';
import type { DumpLevel } from './dump';

export const PRINCIPAL_ID = '01J8A0000000000000000000B1';
export const ACCOUNT_ID = '01J8A0000000000000000000A1';

export const DUMP_LOGINS: Readonly<Record<Exclude<DumpLevel, 'guest'>, string>> = {
  user: 'alice',
  reseller: 'res1',
  admin: 'root',
};

function principal(login: string, level: Level): BootPrincipal {
  return {
    id: PRINCIPAL_ID,
    login,
    level,
    account: ACCOUNT_ID,
    actor: null,
    lang: 'en',
    tz: null,
    totp: false,
  };
}

export const KEEL_PRINCIPALS: Readonly<Record<DumpLevel, BootPrincipal | null>> = {
  guest: null,
  user: principal(DUMP_LOGINS.user, 'user'),
  reseller: principal(DUMP_LOGINS.reseller, 'reseller'),
  admin: principal(DUMP_LOGINS.admin, 'admin'),
};

export function actingAs(principal: BootPrincipal, actor: BootPrincipal): BootPrincipal {
  return {
    ...principal,
    actor: { id: actor.id, login: actor.login, level: actor.level, via: 'su' },
  };
}
