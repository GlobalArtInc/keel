import { test as base, expect, type Page } from '@playwright/test';
import type { Envelope } from '@globalart/keel/meta';

export { expect };

export type E2ERole = 'admin' | 'reseller' | 'user';

export const E2E_URL_VARIABLE = 'KEEL_E2E_URL';
export const E2E_LOGINS: Readonly<Record<E2ERole, string>> = {
  admin: 'root',
  reseller: 'res1',
  user: 'alice',
};
export const E2E_PASSWORD_VARIABLES: Readonly<Record<E2ERole, string>> = {
  admin: 'KEEL_E2E_ROOT_PASSWORD',
  reseller: 'KEEL_E2E_RES1_PASSWORD',
  user: 'KEEL_E2E_ALICE_PASSWORD',
};
export const E2E_SCRIPT = 'hack/e2e.sh';
export const LOGIN_PATH = '/login';
export const LOGOUT_PATH = '/logout';
export const API_PREFIX = '/api/v1';
export const SESSION_COOKIE = '__Host-keel_session';
export const AUTH_LOGIN = 'auth.login';
export const AUTH_LOGOUT = 'auth.logout';

export type Environment = Readonly<Record<string, string | undefined>>;

export interface E2ELogin {
  login: string;
  password: string;
}

export interface E2ESite {
  baseURL: string;
  logins: Readonly<Record<E2ERole, E2ELogin>>;
}

export interface KeelPage {
  login(login: string, password: string): Promise<void>;
  asAdmin(): Promise<void>;
  asReseller(): Promise<void>;
  asUser(): Promise<void>;
  logout(): Promise<void>;
}

export interface KeelApiSession {
  login: string;
  call<T = unknown>(func: string, params?: unknown): Promise<Envelope<T>>;
  logout(): Promise<void>;
}

export interface KeelApi {
  login(login: string, password: string): Promise<KeelApiSession>;
  as(role: E2ERole): Promise<KeelApiSession>;
}

export interface KeelFixtures {
  keelSite: E2ESite;
  keel: KeelPage;
  keelApi: KeelApi;
}

type Host = { process?: { env?: Environment } };

function hostEnvironment(): Environment {
  return (globalThis as Host).process?.env ?? {};
}

function required(env: Environment, variable: string): string {
  const value = env[variable];
  if (value === undefined || value === '') {
    throw new Error(`${variable} is not set; ${E2E_SCRIPT} exports it before running the suite`);
  }
  return value;
}

export function keelBaseUrl(env: Environment = hostEnvironment()): string {
  return required(env, E2E_URL_VARIABLE).replace(/\/+$/, '');
}

export function keelSite(env: Environment = hostEnvironment(), baseURL?: string): E2ESite {
  const logins = {} as Record<E2ERole, E2ELogin>;
  for (const role of Object.keys(E2E_LOGINS) as E2ERole[]) {
    logins[role] = {
      login: E2E_LOGINS[role],
      password: required(env, E2E_PASSWORD_VARIABLES[role]),
    };
  }
  return {
    baseURL: baseURL === undefined ? keelBaseUrl(env) : baseURL.replace(/\/+$/, ''),
    logins,
  };
}

export function onLoginScreen(url: URL): boolean {
  return url.pathname === LOGIN_PATH || url.pathname.startsWith(`${LOGIN_PATH}/`);
}

export function sessionCookieOf(response: Response): string | null {
  for (const header of response.headers.getSetCookie()) {
    const pair = header.split(';')[0]?.trim() ?? '';
    if (pair.startsWith(`${SESSION_COOKIE}=`)) {
      return pair;
    }
  }
  return null;
}

async function post<T>(site: E2ESite, func: string, params: unknown, cookie: string | null) {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (cookie !== null) {
    headers['cookie'] = cookie;
  }
  const response = await fetch(`${site.baseURL}${API_PREFIX}/call/${func}`, {
    method: 'POST',
    headers,
    body: JSON.stringify(params ?? {}),
  });
  const envelope = (await response.json()) as Envelope<T>;
  return { response, envelope };
}

class ApiSession implements KeelApiSession {
  constructor(
    private readonly site: E2ESite,
    readonly login: string,
    private readonly cookie: string,
  ) {}

  async call<T = unknown>(func: string, params?: unknown): Promise<Envelope<T>> {
    return (await post<T>(this.site, func, params, this.cookie)).envelope;
  }

  async logout(): Promise<void> {
    await this.call(AUTH_LOGOUT);
  }
}

export function keelApi(site: E2ESite): KeelApi {
  const login = async (name: string, password: string): Promise<KeelApiSession> => {
    const { response, envelope } = await post(site, AUTH_LOGIN, { login: name, password }, null);
    if (!envelope.ok) {
      throw new Error(`${AUTH_LOGIN} refused ${name}: ${envelope.error.key}`);
    }
    const cookie = sessionCookieOf(response);
    if (cookie === null) {
      throw new Error(`${AUTH_LOGIN} answered without the ${SESSION_COOKIE} cookie`);
    }
    return new ApiSession(site, name, cookie);
  };
  return {
    login,
    as: (role) => login(site.logins[role].login, site.logins[role].password),
  };
}

export function keelPage(page: Page, site: E2ESite): KeelPage {
  const login = async (name: string, password: string): Promise<void> => {
    await page.goto(LOGIN_PATH);
    const form = page.locator('keel-login form[data-step="password"]');
    await form.locator('input[autocomplete="username"]').fill(name);
    await form.locator('input[autocomplete="current-password"]').fill(password);
    await form.locator('button[type="submit"]').click();
    await page.waitForURL((url) => !onLoginScreen(url));
    await page.locator('keel-layout').waitFor();
  };
  const as = async (role: E2ERole): Promise<void> => {
    const { login: name, password } = site.logins[role];
    await login(name, password);
  };
  return {
    login,
    asAdmin: () => as('admin'),
    asReseller: () => as('reseller'),
    asUser: () => as('user'),
    logout: async () => {
      await page.goto(LOGOUT_PATH);
      await page.waitForURL(onLoginScreen);
    },
  };
}

export const keelTest = base.extend<KeelFixtures>({
  keelSite: [
    async ({ baseURL }, use) => {
      await use(keelSite(hostEnvironment(), baseURL));
    },
    { option: true },
  ],
  keel: async ({ page, keelSite: site }, use) => {
    await use(keelPage(page, site));
  },
  keelApi: async ({ keelSite: site }, use) => {
    await use(keelApi(site));
  },
});
