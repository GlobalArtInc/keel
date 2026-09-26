import { Injectable, computed, inject, signal } from '@angular/core';
import { Router, type CanActivateFn, type CanMatchFn, type UrlTree } from '@angular/router';
import { KeelAuth, KeelBoot, KeelI18n } from '@globalart/keel/client';
import type { Boot } from '@globalart/keel/meta';
import { SCREEN_PATH } from '@globalart/keel/renderer';
import { KeelProducts } from './products';

export const LOGIN_PATH = 'login';
export const LOGOUT_PATH = 'logout';
export const SETUP_PATH = 'setup';
export const RESET_PATH = 'reset';
export const VERIFY_PATH = 'verify';
export const RETURN_TO = 'returnTo';
export const GUEST_LANG = 'en';
export const HOME_URL = '/';

export type StartupState = 'loading' | 'failed' | 'ready';

export function firstSegment(url: string): string {
  const end = url.search(/[?#]/);
  const path = end < 0 ? url : url.slice(0, end);
  return path.split('/').find((segment) => segment !== '') ?? '';
}

export function authRoute(url: string): boolean {
  const segment = firstSegment(url);
  return [LOGIN_PATH, LOGOUT_PATH, SETUP_PATH, RESET_PATH, VERIFY_PATH].includes(segment);
}

export function returnable(url: string): boolean {
  return url.startsWith('/') && !url.startsWith('//') && !authRoute(url);
}

export function returnTarget(url: string | null): string {
  return url !== null && returnable(url) ? url : HOME_URL;
}

export function loginUrl(router: Router, returnTo: string | null): UrlTree {
  const target = returnTo === null ? HOME_URL : returnTarget(returnTo);
  const queryParams = target === HOME_URL ? {} : { [RETURN_TO]: target };
  return router.createUrlTree([`/${LOGIN_PATH}`], { queryParams });
}

export function screenUrl(router: Router, func: string): UrlTree {
  return router.createUrlTree([`/${SCREEN_PATH}`, func]);
}

@Injectable({ providedIn: 'root' })
export class KeelStartup {
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly products = inject(KeelProducts);
  private readonly failed = signal<unknown>(null);
  private pending: Promise<void> | null = null;
  private spoken: string | null = null;

  readonly failure = this.failed.asReadonly();
  readonly state = computed<StartupState>(() => {
    if (this.failed() !== null) {
      return 'failed';
    }
    return this.boot.loaded() ? 'ready' : 'loading';
  });

  ready(): Promise<Boot> {
    const failure = this.failed();
    if (failure !== null) {
      return Promise.reject(failure);
    }
    if (this.pending === null) {
      this.pending = this.boot.loaded() ? Promise.resolve() : this.start();
    }
    return this.pending.then(() => this.finish()).catch((error: unknown) => this.fail(error));
  }

  reset(): void {
    this.pending = null;
    this.spoken = null;
    this.failed.set(null);
    this.boot.drop();
  }

  private async start(): Promise<void> {
    await this.boot.load();
  }

  private async finish(): Promise<Boot> {
    const payload = this.boot.payload();
    if (payload === null) {
      throw new TypeError('the boot was dropped while the shell was starting');
    }
    await Promise.all([this.speak(payload), this.products.load(payload)]);
    return payload;
  }

  private fail(error: unknown): never {
    this.pending = null;
    this.failed.set(error ?? new Error('the shell did not start'));
    throw error;
  }

  private async speak(payload: Boot): Promise<void> {
    const lang = payload.principal?.lang ?? GUEST_LANG;
    if (this.spoken !== lang) {
      await this.i18n.load(lang);
      this.spoken = lang;
    }
  }
}

export const keelReady: CanActivateFn & CanMatchFn = async () => {
  const startup = inject(KeelStartup);
  try {
    await startup.ready();
    return true;
  } catch {
    return false;
  }
};

export const keelGuard: CanActivateFn = async (_route, state) => {
  const startup = inject(KeelStartup);
  const router = inject(Router);
  let boot: Boot;
  try {
    boot = await startup.ready();
  } catch {
    return false;
  }
  return boot.principal ? true : loginUrl(router, state.url);
};

export function hasDashboard(boot: Pick<Boot, 'dashboard'> | null): boolean {
  return (boot?.dashboard.length ?? 0) > 0;
}

export const keelHome: CanActivateFn = () => {
  const boot = inject(KeelBoot);
  const router = inject(Router);
  if (hasDashboard(boot.payload())) {
    return true;
  }
  const first = boot.menu().flatMap((group) => group.items)[0];
  return first === undefined ? true : screenUrl(router, first.func);
};

export const keelLogout: CanActivateFn = async () => {
  const auth = inject(KeelAuth);
  const startup = inject(KeelStartup);
  const router = inject(Router);
  let signedOut = false;
  try {
    signedOut = (await auth.logout()).ok;
  } catch {
    signedOut = false;
  }
  if (!signedOut) {
    startup.reset();
  }
  return loginUrl(router, null);
};
