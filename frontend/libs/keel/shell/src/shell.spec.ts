import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router } from '@angular/router';
import { KEEL_EVENT_SOURCE, KeelClient, KeelEvents, type Fetched } from '@globalart/keel/client';
import type { Boot } from '@globalart/keel/meta';
import {
  BASE_CHROMA,
  KeelShellComponent,
  KeelStartup,
  chromaScale,
  keelProviders,
  plainClick,
} from '@globalart/keel/shell';
import { KeelMockClient, QuietEventSource } from '@globalart/keel/testing';
import { PhxThemeService } from '@globalart/phoenix-ui';
import { throwError, type Observable } from 'rxjs';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const BOOT: Boot = {
  ...(JSON.parse(readFileSync(join(FIXTURES, 'boot-none.json'), 'utf8')) as Boot),
  dashboard: [],
};

const MESSAGES = {
  menu: { access: 'Users and access', monitor: 'Monitoring', system: 'System' },
  keel: {
    account: { list: { title: 'Accounts' } },
    principal: { list: { title: 'Principals' } },
    product: { list: { title: 'Products' } },
    doctor: { run: { title: 'Health' } },
    logs: { stream: { title: 'Log' } },
    menu: { users: 'Users', logs: 'Logs', install: 'Installation' },
    shell: { boot_failed: 'The installation did not answer; reload the page' },
  },
  phx: { retry: 'Try again', loading: 'Loading' },
};

class FlakyClient extends KeelMockClient {
  down = true;

  override boot(): Observable<Fetched<Boot> | null> {
    return this.down ? throwError(() => new Error('connection refused')) : super.boot();
  }
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

function configure(client: KeelMockClient): ComponentFixture<KeelShellComponent> {
  TestBed.configureTestingModule({
    providers: [
      ...keelProviders(),
      { provide: KeelClient, useValue: client },
      {
        provide: KEEL_EVENT_SOURCE,
        useValue: (url: string, init: EventSourceInit) => new QuietEventSource(url, init),
      },
    ],
  });
  const fixture = TestBed.createComponent(KeelShellComponent);
  fixture.detectChanges();
  return fixture;
}

describe('KeelShellComponent', () => {
  beforeEach(() => QuietEventSource.reset());

  afterEach(() => TestBed.resetTestingModule());

  it('shows the loader until the boot answers, the failure with a retry when it does not, and the layout with the menu once it does', async () => {
    const client = new FlakyClient({ boot: BOOT, catalogue: { lang: 'en', messages: MESSAGES } });
    const fixture = configure(client);
    const router = TestBed.inject(Router);
    const element: HTMLElement = fixture.nativeElement;
    expect(element.getAttribute('data-startup')).toBe('loading');
    expect(element.querySelector('phx-loader')).not.toBeNull();

    const loads = jest.spyOn(client, 'boot');
    const failed = router.navigateByUrl('/f/keel.account.list');
    await expect(failed).resolves.toBe(false);
    await settle(fixture);
    expect(loads).toHaveBeenCalledTimes(1);
    expect(QuietEventSource.opened).toHaveLength(0);
    expect(TestBed.inject(KeelEvents).state().phase).toBe('idle');
    expect(element.getAttribute('data-startup')).toBe('failed');
    expect(element.querySelector('phx-error .text')?.textContent?.trim()).toBe(
      'keel.shell.boot_failed',
    );
    expect(TestBed.inject(KeelStartup).failure()).toBeInstanceOf(Error);
    expect(element.querySelector('router-outlet')).toBeNull();
    await expect(router.navigateByUrl('/')).resolves.toBe(false);
    expect(loads).toHaveBeenCalledTimes(1);

    client.down = false;
    const retry = element.querySelector<HTMLButtonElement>('phx-error button');
    expect(retry?.textContent?.trim()).toBe('Retry');
    retry?.click();
    await settle(fixture);
    await settle(fixture);
    expect(loads).toHaveBeenCalledTimes(2);
    expect(element.getAttribute('data-startup')).toBe('ready');
    expect(router.url).toBe('/f/keel.principal.list');
    expect(TestBed.inject(KeelStartup).failure()).toBeNull();
    expect(QuietEventSource.opened).toHaveLength(1);
    expect(QuietEventSource.last()?.url).toBe('/api/v1/events');
    expect(QuietEventSource.last()?.init).toEqual({ withCredentials: true });
    expect(TestBed.inject(KeelEvents).state().phase).toBe('connecting');

    const theme = TestBed.inject(PhxThemeService);
    expect(theme.hue()).toBe(BOOT.brand.hue);
    expect(theme.chroma()).toBeCloseTo(BOOT.brand.chroma / BASE_CHROMA);
    expect(chromaScale(BASE_CHROMA)).toBe(1);

    const items = [...element.querySelectorAll('keel-layout phx-sidebar-item')];
    expect(items.map((item) => item.querySelector('.label')?.textContent)).toEqual([
      'Users',
      'Logs',
      'Health',
      'Installation',
    ]);
    expect(items.map((item) => item.querySelector('a')?.getAttribute('href'))).toEqual([
      '/f/keel.principal.list',
      '/f/keel.logs.stream',
      '/f/keel.doctor.run',
      '/f/keel.product.list',
    ]);
    expect(items.map((item) => item.querySelector('a')?.classList.contains('item_active'))).toEqual(
      [true, false, false, false],
    );
    expect(element.querySelector('phx-sidebar nav.menu')?.getAttribute('aria-label')).toBe(
      'System',
    );

    const link = items[1].querySelector<HTMLAnchorElement>('a');
    const click = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
    link?.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    await settle(fixture);
    expect(router.url).toBe('/f/keel.logs.stream');
    expect(items[1].querySelector('a')?.classList.contains('item_active')).toBe(true);

    const withKey = new MouseEvent('click', { bubbles: true, cancelable: true, metaKey: true });
    items[2].dispatchEvent(withKey);
    expect(withKey.defaultPrevented).toBe(false);
    await settle(fixture);
    expect(router.url).toBe('/f/keel.logs.stream');
    expect(plainClick(withKey)).toBe(false);
    expect(plainClick(new MouseEvent('click', { button: 1 }))).toBe(false);
    expect(plainClick(new MouseEvent('click', { button: 0 }))).toBe(true);
  });
});
