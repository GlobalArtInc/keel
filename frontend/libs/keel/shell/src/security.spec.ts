import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  KeelBoot,
  KeelClient,
  KeelI18n,
  type Catalogue,
  type Fetched,
} from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope, Messages } from '@globalart/keel/meta';
import { KeelScreenComponent, injectKeelViews } from '@globalart/keel/renderer';
import {
  KeelSecurityComponent,
  KeelSettingsValueComponent,
  PASSWORD_SET,
  SHELL_VIEWS,
  TOTP_CONFIRM,
  TOTP_DISABLE,
  TOTP_ENROLL,
  keelProviders,
  ownPasswordDescriptor,
  provideKeelShellViews,
  qrOf,
  viewBoxOf,
} from '@globalart/keel/shell';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxModalService, PhxToastService, providePhoenixIcons } from '@globalart/phoenix-ui';
import { of, type Observable } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const SNAPSHOTS = resolve(__dirname, '../../../../../crates/system/tests/snapshots');
const VIEWS = resolve(__dirname, '../../../../apps/shell/views.json');
const SECRET = 'JBSWY3DPEHPK3PXP';
const URI = `otpauth://totp/keel:res1?secret=${SECRET}&issuer=keel`;
const CODES = [
  'A2B3C4D5E6',
  'F7G8H9J2K3',
  'L4M5N6P7Q8',
  'R9S2T3U4V5',
  'W6X7Y8Z2A3',
  'B4C5D6E7F8',
  'G9H2J3K4L5',
  'M6N7P8Q9R2',
  'S3T4U5V6W7',
  'X8Y9Z2A3B4',
];

function load<T>(name: string): T {
  return JSON.parse(readFileSync(join(FIXTURES, name), 'utf8')) as T;
}

function snapshot(func: string, level: string): Descriptor {
  const text = readFileSync(join(SNAPSHOTS, `${func}@${level}.snap`), 'utf8');
  const body = text.slice(text.indexOf('\n---\n', text.indexOf('---\n') + 4) + 5);
  return JSON.parse(body) as Descriptor;
}

const ENROLL = snapshot(TOTP_ENROLL, 'user');
const PASSWORD = snapshot(PASSWORD_SET, 'admin');

const MESSAGES: Messages = {
  menu: { system: 'System' },
  auth: {
    password: {
      set: { title: 'Change password' },
      current: 'Current password',
      password: 'New password',
    },
    totp: {
      code: 'Code',
      enroll: { title: 'Set up a second factor' },
      disable: { title: 'Disable the second factor' },
    },
    error: {
      code: 'The code is wrong',
      current_required: 'The current password is required',
      totp_enrolled: 'A second factor is already enrolled; disable it first',
    },
  },
  keel: {
    brand: 'Keel',
    page: { main: 'Main' },
    level: { reseller: 'Reseller' },
    account: { list: { title: 'Accounts' } },
    principal: { list: { title: 'Principals' } },
    product: { list: { title: 'Products' } },
    action: { save: 'Save', retry: 'Retry', done: '{action} completed', finish: 'Finish' },
    error: { request: '{message} (request {id})', forbidden: 'You are not allowed to do this' },
    shell: {
      user: {
        language: 'Language',
        theme: 'Theme',
        sessions: 'Sessions',
        tokens: 'API tokens',
        security: 'Security',
        logout: 'Log out',
      },
      language: { en: 'English', ru: 'Russian' },
      rail: { notifications: 'Notifications', tasks: 'Tasks', help: 'Help', later: 'Later' },
      security: {
        title: 'Security',
        password: 'Password',
        password_hint: 'Change the password of your account',
        totp: 'Second factor',
        enrolled: 'A second factor is enrolled',
        not_enrolled: 'No second factor is enrolled',
        enroll: 'Set up',
        confirm: 'Confirm',
        disable: 'Disable',
        secret: 'Secret',
        secret_hint: 'Scan the code or enter the secret, then confirm with a code',
        recovery: 'Recovery codes',
        recovery_hint: 'Store these codes now: they are shown only once',
      },
    },
  },
};

function bootWith(totp: boolean): Boot {
  const boot = load<Boot>('boot.json');
  if (boot.principal) {
    boot.principal.totp = totp;
  }
  return boot;
}

function ok(data: unknown, requestId = 'req-security'): Envelope {
  return { ok: true, data, meta: { requestId } };
}

function refused(key: string, path?: string): Envelope {
  const error: Envelope = {
    ok: false,
    error: {
      code: path === undefined ? 'unauthenticated' : 'validation',
      key: path === undefined ? key : 'keel.error.validation',
      requestId: 'req-refused',
    },
  };
  if (!error.ok && path !== undefined) {
    error.error.fields = [{ path, key }];
  }
  return error;
}

class SecurityClient extends KeelMockClient {
  current: Boot;
  boots = 0;

  constructor(
    boot: Boot,
    answers: Record<string, Envelope | Envelope[]>,
    private readonly after: Partial<Record<string, Boot>> = {},
  ) {
    super({ boot, descriptors: [ENROLL, PASSWORD], answers });
    this.current = boot;
  }

  override boot(): Observable<Fetched<Boot> | null> {
    this.boots += 1;
    return of({ body: this.current, etag: `"${this.current.meta}-${this.boots}"` });
  }

  override catalogue(lang: string): Observable<Fetched<Catalogue> | null> {
    return of({ body: { lang, messages: MESSAGES }, etag: null });
  }

  override call<T = unknown>(func: string, params?: unknown): Observable<Envelope<T>> {
    const answered = super.call<T>(func, params);
    const next = this.after[func];
    if (next !== undefined) {
      this.current = next;
    }
    return answered;
  }
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

async function render(client: SecurityClient): Promise<ComponentFixture<KeelScreenComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelScreenComponent],
    providers: [
      providePhoenixIcons(),
      provideKeelShellViews(),
      provideRouter([{ path: 'f/:func', children: [] }]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const fixture = TestBed.createComponent(KeelScreenComponent);
  fixture.componentRef.setInput('func', TOTP_ENROLL);
  fixture.detectChanges();
  await settle(fixture);
  await settle(fixture);
  return fixture;
}

function overlay(): HTMLElement {
  const container = document.querySelector<HTMLElement>('.cdk-overlay-container');
  if (container === null) {
    throw new Error('no overlay container');
  }
  return container;
}

function click(target: Element | null): void {
  if (!(target instanceof HTMLElement)) {
    throw new Error('nothing to click');
  }
  target.click();
}

function type(input: Element | null, value: string): void {
  if (!(input instanceof HTMLInputElement)) {
    throw new Error('no input to type into');
  }
  input.value = value;
  input.dispatchEvent(new Event('input'));
}

function submit(form: Element | null): void {
  if (!(form instanceof HTMLFormElement)) {
    throw new Error('no form to submit');
  }
  form.dispatchEvent(new Event('submit'));
}

function totpSection(element: HTMLElement): HTMLElement | null {
  return element.querySelector('[data-section="totp"]');
}

describe('the security page of the shell', () => {
  afterEach(() => {
    TestBed.inject(PhxModalService).closeAll();
    TestBed.inject(PhxToastService).closeAll();
    TestBed.resetTestingModule();
  });

  it('enrolment shows the secret and a QR, the confirm shows the recovery codes once', async () => {
    const client = new SecurityClient(
      bootWith(false),
      {
        [TOTP_ENROLL]: ok({ secret: SECRET, uri: URI }),
        [TOTP_CONFIRM]: ok({ recoveryCodes: CODES }),
      },
      { [TOTP_CONFIRM]: bootWith(true) },
    );
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-action')).toBeNull();
    const page = element.querySelector('keel-security');
    expect(page?.getAttribute('data-security')).toBe(TOTP_ENROLL);
    expect(element.querySelector('phx-page-header .title__text')?.textContent).toBe('Security');
    expect(totpSection(element)?.getAttribute('data-enrolled')).toBe('false');
    expect(totpSection(element)?.querySelector('.description')?.textContent).toBe(
      'No second factor is enrolled',
    );
    expect(totpSection(element)?.querySelector('button.disable')).toBeNull();

    click(totpSection(element)?.querySelector('button.enroll') ?? null);
    await settle(fixture);
    expect(client.callsOf(TOTP_ENROLL).map((call) => call.params)).toEqual([{}]);
    const modal = overlay().querySelector<HTMLElement>('phx-modal keel-totp-enroll');
    expect(modal?.getAttribute('data-stage')).toBe('confirm');
    expect(overlay().querySelector('phx-modal .title')?.textContent).toBe('Set up a second factor');
    expect(modal?.querySelector('[data-secret] .value-text')?.textContent?.trim()).toBe(SECRET);
    expect(modal?.querySelector('[data-secret] phx-clipboard button')).not.toBeNull();
    expect(modal?.querySelector('[data-copy="uri"] button')?.getAttribute('aria-label')).toBe(
      'Copy',
    );
    const qr = modal?.querySelector('svg[data-qr]');
    const expected = qrOf(URI);
    expect(qr?.getAttribute('aria-label')).toBe(URI);
    expect(qr?.getAttribute('viewBox')).toBe(viewBoxOf(expected));
    expect(qr?.querySelector('path')?.getAttribute('d')).toBe(expected.path);
    expect(expected.size).toBeGreaterThan(20);
    expect(expected.path).toMatch(/^M0 0h1v1h-1z/);
    expect(modal?.querySelector('[data-recovery]')).toBeNull();

    type(modal?.querySelector('phx-input input') ?? null, '123456');
    submit(modal?.querySelector('form') ?? null);
    await settle(fixture);
    expect(client.callsOf(TOTP_CONFIRM).map((call) => call.params)).toEqual([{ code: '123456' }]);
    expect(modal?.getAttribute('data-stage')).toBe('recovery');
    const listed = [...(modal?.querySelectorAll('[data-recovery] li') ?? [])].map((item) =>
      item.textContent?.trim(),
    );
    expect(listed).toEqual(CODES);
    expect(modal?.querySelector('phx-banner .title')?.textContent).toBe('Recovery codes');
    expect(modal?.querySelector('[data-copy="recovery"] button')).not.toBeNull();
    expect(modal?.querySelector('form')).toBeNull();

    const boots = client.boots;
    click(modal?.querySelector('button.finish') ?? null);
    await settle(fixture);
    await settle(fixture);
    expect(overlay().querySelector('keel-totp-enroll')).toBeNull();
    expect(client.boots).toBe(boots + 1);
    expect(totpSection(element)?.getAttribute('data-enrolled')).toBe('true');
    expect(totpSection(element)?.querySelector('.description')?.textContent).toBe(
      'A second factor is enrolled',
    );
    expect(totpSection(element)?.querySelector('button.enroll')).toBeNull();
    expect(document.body.textContent).not.toContain(CODES[0]);
    expect(document.body.textContent).not.toContain(SECRET);
  });

  it('a wrong code stays in the modal with the refusal, and the disable modal posts auth.totp.disable { code }', async () => {
    const client = new SecurityClient(
      bootWith(false),
      {
        [TOTP_ENROLL]: ok({ secret: SECRET, uri: URI }),
        [TOTP_CONFIRM]: refused('auth.error.code', 'code'),
        [TOTP_DISABLE]: [
          refused('auth.error.code'),
          ok({ principal: '01J8A0000000000000000000B1', disabled: true }),
        ],
      },
      { [TOTP_DISABLE]: bootWith(false) },
    );
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    click(totpSection(element)?.querySelector('button.enroll') ?? null);
    await settle(fixture);
    const enroll = overlay().querySelector<HTMLElement>('keel-totp-enroll');
    submit(enroll?.querySelector('form') ?? null);
    await settle(fixture);
    expect(client.callsOf(TOTP_CONFIRM)).toHaveLength(0);
    type(enroll?.querySelector('phx-input input') ?? null, '000000');
    submit(enroll?.querySelector('form') ?? null);
    await settle(fixture);
    expect(client.callsOf(TOTP_CONFIRM)).toHaveLength(1);
    expect(enroll?.getAttribute('data-stage')).toBe('confirm');
    expect(enroll?.querySelector('phx-field-error')?.textContent?.trim()).toBe('The code is wrong');
    TestBed.inject(PhxModalService).closeAll();
    await settle(fixture);
    await settle(fixture);
    expect(overlay().querySelector('keel-totp-enroll')).toBeNull();
    expect(totpSection(element)?.getAttribute('data-enrolled')).toBe('false');

    client.current = bootWith(true);
    await TestBed.inject(KeelBoot).load();
    await settle(fixture);
    expect(totpSection(element)?.getAttribute('data-enrolled')).toBe('true');
    click(totpSection(element)?.querySelector('button.disable') ?? null);
    await settle(fixture);
    const disable = overlay().querySelector<HTMLElement>('phx-modal keel-totp-disable');
    expect(overlay().querySelector('phx-modal .title')?.textContent).toBe(
      'Disable the second factor',
    );
    type(disable?.querySelector('phx-input input') ?? null, '111111');
    submit(disable?.querySelector('form') ?? null);
    await settle(fixture);
    expect(disable?.querySelector('phx-field-error')?.textContent?.trim()).toBe(
      'The code is wrong (request req-refused)',
    );
    expect(overlay().querySelector('keel-totp-disable')).not.toBeNull();

    type(disable?.querySelector('phx-input input') ?? null, '222222');
    submit(disable?.querySelector('form') ?? null);
    await settle(fixture);
    await settle(fixture);
    expect(client.callsOf(TOTP_DISABLE).map((call) => call.params)).toEqual([
      { code: '111111' },
      { code: '222222' },
    ]);
    expect(overlay().querySelector('keel-totp-disable')).toBeNull();
    expect(totpSection(element)?.getAttribute('data-enrolled')).toBe('false');
  });

  it('the password form posts auth.password.set { current, password } without the principal field', async () => {
    const client = new SecurityClient(bootWith(false), {
      [PASSWORD_SET]: [
        refused('auth.error.current_required', 'current'),
        ok({ principal: '01J8A0000000000000000000B1' }, 'req-changed'),
      ],
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    const section = element.querySelector('[data-section="password"]');
    expect(element.querySelector('keel-security')?.getAttribute('data-password')).toBe('ready');
    expect(section?.querySelector('.title')?.textContent).toBe('Password');
    expect(section?.querySelector('.description')?.textContent).toBe(
      'Change the password of your account',
    );
    const labels = [...(section?.querySelectorAll('phx-form-row .label__text') ?? [])].map(
      (label) => label.textContent,
    );
    expect(labels).toEqual(['Current password', 'New password']);
    expect(ownPasswordDescriptor(PASSWORD).ui?.pages).toEqual([
      { id: 'main', fields: ['current', 'password'] },
    ]);

    const inputs = section?.querySelectorAll('phx-password input') ?? [];
    type(inputs[1] ?? null, 'correct-horse-battery');
    click(section?.querySelector('button.save') ?? null);
    await settle(fixture);
    expect(client.callsOf(PASSWORD_SET).map((call) => call.params)).toEqual([
      { current: null, password: 'correct-horse-battery' },
    ]);
    expect(section?.querySelector('phx-field-error')?.textContent?.trim()).toBe(
      'The current password is required',
    );

    type(inputs[0] ?? null, 'old-password');
    click(section?.querySelector('button.save') ?? null);
    await settle(fixture);
    await settle(fixture);
    expect(client.callsOf(PASSWORD_SET).map((call) => call.params)).toEqual([
      { current: null, password: 'correct-horse-battery' },
      { current: 'old-password', password: 'correct-horse-battery' },
    ]);
    expect(overlay().querySelector('phx-toast')?.textContent).toContain(
      'Change password completed',
    );
    const fresh = section?.querySelectorAll<HTMLInputElement>('phx-password input') ?? [];
    expect([...fresh].map((input) => input.value)).toEqual(['', '']);
  });

  it('frontend/apps/shell/views.json names exactly the two views the shell registers', () => {
    const listed = JSON.parse(readFileSync(VIEWS, 'utf8')) as string[];
    expect(listed).toEqual(SHELL_VIEWS.map((view) => view.func));
    expect(listed).toEqual(['settings.set', 'auth.totp.enroll']);
    TestBed.configureTestingModule({ providers: [provideKeelShellViews()] });
    const views = TestBed.runInInjectionContext(injectKeelViews);
    expect(views.map((view) => view.func)).toEqual(listed);
    expect(views.map((view) => view.component)).toEqual([
      KeelSettingsValueComponent,
      KeelSecurityComponent,
    ]);
  });

  it('the user menu reaches the security page through the routes of the shell', async () => {
    const client = new SecurityClient(bootWith(false), {});
    TestBed.configureTestingModule({
      providers: [...keelProviders(), { provide: KeelClient, useValue: client }],
    });
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/f/keel.account.list');
    await settle(harness.fixture);
    const element: HTMLElement = harness.fixture.nativeElement;
    click(element.querySelector('phx-rail-item[data-rail="user"] button'));
    await settle(harness.fixture);
    const item = [...overlay().querySelectorAll<HTMLElement>('phx-dropdown-item')].find((entry) =>
      entry.textContent?.includes('Security'),
    );
    click(item ?? null);
    await settle(harness.fixture);
    await settle(harness.fixture);
    expect(TestBed.inject(Router).url).toBe(`/f/${TOTP_ENROLL}`);
    expect(element.querySelector('keel-screen')?.getAttribute('data-phase')).toBe('view');
    expect(element.querySelector('keel-security')).not.toBeNull();
  });
});
