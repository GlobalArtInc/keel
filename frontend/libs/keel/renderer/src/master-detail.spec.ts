import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KEEL_SCREEN_DEPTH, KeelScreenComponent } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import { By } from '@angular/platform-browser';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const DUMP = resolve(__dirname, '../../../../../contracts/dump');
const ZONES = 'dns.zone.list';
const RECORDS = 'dns.record.list';
const ZONE = '01J8A0000000000000000000Z1';
const OTHER = '01J8A0000000000000000000Z2';

function dumped(name: string): Descriptor {
  return JSON.parse(
    readFileSync(join(DUMP, 'user/functions', `${name}.json`), 'utf8'),
  ) as Descriptor;
}

const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;

const MESSAGES = {
  dns: {
    zone: { list: { title: 'Zones' }, column: { name: 'Name' } },
    record: { list: { title: 'Records' }, column: { name: 'Name' } },
  },
  keel: { detail: { empty: 'Select a row to see what it holds' } },
};

function page(rows: Record<string, unknown>[]): Envelope {
  return {
    ok: true,
    data: { data: rows, total: rows.length, totals: null },
    meta: { requestId: 'req-split' },
  };
}

function client(): KeelMockClient {
  return new KeelMockClient({
    boot: BOOT,
    descriptors: [dumped(ZONES), dumped(RECORDS)],
    catalogue: { lang: 'en', messages: MESSAGES },
    answers: {
      [ZONES]: page([
        { id: ZONE, name: 'chosen.example', state: 'active', serial: 1 },
        { id: OTHER, name: 'other.example', state: 'active', serial: 1 },
      ]),
      [RECORDS]: page([{ id: 'r1', name: 'www', type: 'A', value: '192.0.2.7', ttl: 3600 }]),
    },
  });
}

async function settle(fixture: ComponentFixture<unknown>): Promise<void> {
  fixture.detectChanges();
  await fixture.whenStable();
  await new Promise((done) => setTimeout(done, 0));
  fixture.detectChanges();
}

interface Opened {
  element: HTMLElement;
  harness: RouterTestingHarness;
}

async function open(mock: KeelMockClient, url: string): Promise<Opened> {
  TestBed.configureTestingModule({
    providers: [
      providePhoenixIcons(),
      provideRouter(
        [{ path: 'f/:func', component: KeelScreenComponent }],
        withComponentInputBinding(),
      ),
      { provide: KeelClient, useValue: mock },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en');
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl(url, KeelScreenComponent);
  await settle(harness.fixture);
  await settle(harness.fixture);
  const element = harness.routeNativeElement;
  if (element === null) {
    throw new Error('the screen did not render');
  }
  return { element, harness };
}

function detail(element: HTMLElement): Element | null {
  return element.querySelector(`keel-master-detail .detail keel-list[data-list="${RECORDS}"]`);
}

function zoneRows(element: HTMLElement): HTMLElement[] {
  return [...element.querySelectorAll<HTMLElement>(`keel-list[data-list="${ZONES}"] .body .row`)];
}

describe('keel-master-detail', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('the master list sits at the screen depth and the detail one level below it', async () => {
    const { harness } = await open(client(), `/f/${ZONES}?selected=${ZONE}`);
    const depthOf = (selector: string): number =>
      harness.fixture.debugElement.query(By.css(selector)).injector.get(KEEL_SCREEN_DEPTH);
    expect(depthOf('keel-master-detail keel-list.master')).toBe(0);
    expect(depthOf('keel-master-detail .detail keel-list')).toBe(1);
  });

  it('master_detail_keeps_the_selection_in_the_route', async () => {
    const first = client();
    const { element, harness } = await open(first, `/f/${ZONES}`);
    expect(element.querySelector(`keel-master-detail[data-master="${ZONES}"]`)).not.toBeNull();
    expect(detail(element)).toBeNull();
    expect(element.querySelector('keel-master-detail .detail phx-empty')?.textContent).toContain(
      'Select a row to see what it holds',
    );

    zoneRows(element)[0]?.click();
    await settle(harness.fixture);
    await settle(harness.fixture);
    expect(TestBed.inject(Router).url).toBe(`/f/${ZONES}?selected=${ZONE}`);
    expect(detail(element)).not.toBeNull();
    expect(first.callsOf(ZONES)).toHaveLength(1);
    expect(first.callsOf(RECORDS).map((call) => call.params)).toEqual([
      expect.objectContaining({ scope: { zone_id: ZONE } }),
    ]);

    TestBed.resetTestingModule();
    const reloaded = client();
    const { element: again } = await open(reloaded, `/f/${ZONES}?selected=${ZONE}`);
    expect(detail(again)).not.toBeNull();
    expect(again.querySelector('keel-master-detail .detail')?.getAttribute('data-selected')).toBe(
      ZONE,
    );
    expect(reloaded.callsOf(ZONES).map((call) => call.params)).toEqual([
      expect.not.objectContaining({ scope: expect.anything() }),
    ]);
    expect(reloaded.callsOf(RECORDS).map((call) => call.params)).toEqual([
      expect.objectContaining({ scope: { zone_id: ZONE } }),
    ]);
  });
});
