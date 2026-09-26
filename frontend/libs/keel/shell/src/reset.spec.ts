import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AUTH_ADDRESS_VERIFY,
  AUTH_RESET_CONFIRM,
  AUTH_RESET_REQUEST,
  KEEL_EVENT_SOURCE,
  KeelClient,
} from '@globalart/keel/client';
import type { Boot, Envelope, ErrorCode } from '@globalart/keel/meta';
import { keelProviders } from '@globalart/keel/shell';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const TOKEN = `kpr_${'ab'.repeat(32)}`;
const CHECK = `kav_${'cd'.repeat(32)}`;

const MESSAGES = {
  keel: {
    brand: 'Keel',
    shell: {
      login: {
        title: 'Sign in',
        login: 'Login',
        password: 'Password',
        forgot: 'Forgot the password?',
      },
      reset: {
        title: 'Reset the password',
        intro: 'Enter your login.',
        login: 'Login',
        submit: 'Send the link',
        sent: 'If the login has a confirmed email address, a link is on its way.',
        password: 'New password',
        save: 'Set the password',
        done: 'The password is changed. Sign in with the new password.',
        failed: 'The request failed; try again',
        to_login: 'Back to sign in',
      },
      verify: {
        title: 'Confirm the email address',
        pending: 'Confirming the address',
        done: 'The address {value} is confirmed',
        failed: 'The confirmation link is not valid',
        continue: 'Continue',
      },
    },
  },
  auth: {
    error: {
      reset_limited: 'Too many reset requests; try again later',
      reset_token: 'The reset link is not valid any more; ask for a new one',
      verify_token: 'The confirmation link is not valid any more; set the address again',
      password_short: 'The password must be at least {min} characters long',
    },
  },
};

function guestBoot(): Boot {
  const boot = JSON.parse(readFileSync(join(FIXTURES, 'boot-none.json'), 'utf8')) as Boot;
  return { ...boot, principal: null, dashboard: [] };
}

function ok(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-reset' } };
}

function refused(
  key: string,
  code: ErrorCode,
  field?: string,
  params?: Record<string, unknown>,
): Envelope {
  if (field === undefined) {
    return { ok: false, error: { code, key, requestId: 'req-refused' } };
  }
  return {
    ok: false,
    error: {
      code,
      key: 'keel.error.validation',
      requestId: 'req-refused',
      fields: [{ path: field, key, params }],
    },
  };
}

function configure(answers: Record<string, Envelope | Envelope[]>): KeelMockClient {
  const client = new KeelMockClient({
    boot: guestBoot(),
    catalogue: { lang: 'en', messages: MESSAGES },
    answers,
  });
  TestBed.configureTestingModule({
    providers: [
      ...keelProviders({ products: [] }),
      { provide: KeelClient, useValue: client },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  return client;
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function type(root: Element, selector: string, value: string): void {
  const input = root.querySelector<HTMLInputElement>(selector);
  if (input === null) {
    throw new Error(`no input matches ${selector}`);
  }
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

function submit(root: Element, step: string): void {
  const form = root.querySelector<HTMLFormElement>(`form[data-step="${step}"]`);
  if (form === null) {
    throw new Error(`the ${step} step is not shown`);
  }
  form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
}

function text(root: Element, selector: string): string | null {
  return root.querySelector(selector)?.textContent?.trim() ?? null;
}

describe('the password reset pages', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('the login page links to the reset form, which posts auth.password.reset.request and shows the same answer for any login', async () => {
    const client = configure({
      [AUTH_RESET_REQUEST]: [refused('auth.error.reset_limited', 'rate_limited'), ok({})],
    });
    const harness = await RouterTestingHarness.create('/login');
    const router = TestBed.inject(Router);
    const element: HTMLElement = harness.fixture.nativeElement;
    const forgot = element.querySelector<HTMLAnchorElement>('keel-login a.forgot');
    expect(forgot?.textContent?.trim()).toBe('Forgot the password?');
    forgot?.click();
    await settle(harness.fixture);
    expect(router.url).toBe('/reset');
    const reset = element.querySelector('keel-reset');
    expect(reset?.getAttribute('data-stage')).toBe('request');
    expect(text(element, 'phx-logon .title')).toBe('Reset the password');

    submit(element, 'request');
    await settle(harness.fixture);
    expect(client.callsOf(AUTH_RESET_REQUEST)).toHaveLength(0);

    type(element, 'form[data-step="request"] phx-input input', ' nobody ');
    submit(element, 'request');
    await settle(harness.fixture);
    expect(text(element, 'keel-reset phx-banner .text')).toBe(
      'Too many reset requests; try again later',
    );
    expect(reset?.getAttribute('data-stage')).toBe('request');

    submit(element, 'request');
    await settle(harness.fixture);
    expect(client.callsOf(AUTH_RESET_REQUEST).map((call) => call.params)).toEqual([
      { login: 'nobody' },
      { login: 'nobody' },
    ]);
    expect(reset?.getAttribute('data-stage')).toBe('sent');
    expect(text(element, 'keel-reset phx-banner .text')).toBe(
      'If the login has a confirmed email address, a link is on its way.',
    );
    element.querySelector<HTMLAnchorElement>('keel-reset a.to-login')?.click();
    await settle(harness.fixture);
    expect(router.url).toBe('/login');
  });

  it('the link of the mail posts the new password with its token, shows a refusal and ends with the way back to sign in', async () => {
    const client = configure({
      [AUTH_RESET_CONFIRM]: [
        refused('auth.error.password_short', 'validation', 'password', { min: 12 }),
        ok({ login: 'alice' }),
      ],
    });
    const harness = await RouterTestingHarness.create(`/reset?token=${TOKEN}`);
    const element: HTMLElement = harness.fixture.nativeElement;
    const reset = element.querySelector('keel-reset');
    expect(reset?.getAttribute('data-stage')).toBe('confirm');

    type(element, 'form[data-step="confirm"] phx-password input', 'short');
    submit(element, 'confirm');
    await settle(harness.fixture);
    expect(text(element, 'keel-reset phx-banner .text')).toBe(
      'The password must be at least 12 characters long',
    );

    type(element, 'form[data-step="confirm"] phx-password input', 'correct-horse-battery');
    submit(element, 'confirm');
    await settle(harness.fixture);
    expect(client.callsOf(AUTH_RESET_CONFIRM).map((call) => call.params)).toEqual([
      { token: TOKEN, password: 'short' },
      { token: TOKEN, password: 'correct-horse-battery' },
    ]);
    expect(reset?.getAttribute('data-stage')).toBe('done');
    expect(text(element, 'keel-reset phx-banner .text')).toBe(
      'The password is changed. Sign in with the new password.',
    );
    expect(text(element, 'keel-reset a.to-login')).toBe('Back to sign in');
  });

  it('the confirmation link of an address posts auth.address.verify with its token and shows the outcome', async () => {
    const client = configure({
      [AUTH_ADDRESS_VERIFY]: [
        ok({ value: 'alice@example.org' }),
        refused('auth.error.verify_token', 'validation', 'token'),
      ],
    });
    const harness = await RouterTestingHarness.create(`/verify?token=${CHECK}`);
    await settle(harness.fixture);
    const element: HTMLElement = harness.fixture.nativeElement;
    const verify = element.querySelector('keel-verify');
    expect(client.callsOf(AUTH_ADDRESS_VERIFY).map((call) => call.params)).toEqual([
      { token: CHECK },
    ]);
    expect(verify?.getAttribute('data-phase')).toBe('done');
    expect(text(element, 'keel-verify phx-banner .text')).toBe(
      'The address alice@example.org is confirmed',
    );

    await harness.navigateByUrl(`/verify?token=${CHECK}x`);
    await settle(harness.fixture);
    expect(element.querySelector('keel-verify')?.getAttribute('data-phase')).toBe('failed');
    expect(text(element, 'keel-verify phx-banner .text')).toBe(
      'The confirmation link is not valid any more; set the address again',
    );
    expect(text(element, 'keel-verify a.continue')).toBe('Continue');
  });
});
