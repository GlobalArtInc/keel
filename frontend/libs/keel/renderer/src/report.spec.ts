import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { TestBed, type ComponentFixture } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import type { Boot, Descriptor, Envelope } from '@globalart/keel/meta';
import { KeelReportComponent } from '@globalart/keel/renderer';
import { KeelMockClient } from '@globalart/keel/testing';
import { PhxChartComponent, providePhoenixIcons } from '@globalart/phoenix-ui';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const DUMP = resolve(__dirname, '../../../../../contracts/dump');
const REPORT_NAME = 'note.report.tags';
const SERIES_NAME = 'note.report.series';

function dumped(name: string): Descriptor {
  return JSON.parse(
    readFileSync(join(DUMP, 'user/functions', `${name}.json`), 'utf8'),
  ) as Descriptor;
}

const REPORT = dumped(REPORT_NAME);
const SERIES = dumped(SERIES_NAME);
const EXPORT = JSON.parse(
  readFileSync(join(FIXTURES, 'descriptors/keel.export.json'), 'utf8'),
) as Descriptor;
const FIXTURE = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;
const PRINCIPAL = FIXTURE.principal ?? null;
const BOOT: Boot = {
  ...FIXTURE,
  principal: PRINCIPAL === null ? null : { ...PRINCIPAL, lang: 'en-GB', tz: 'Asia/Tokyo' },
};

const MESSAGES = {
  keel: {
    action: { apply: 'Apply', export: 'Export', retry: 'Retry' },
    export: { format: { csv: 'CSV' } },
    report: { empty: 'No data for this period' },
  },
  note: {
    report: {
      tags: { title: 'Tag report' },
      band: { tags: 'Tags' },
      column: { tag: 'Tag', notes: 'Notes' },
      chart: {
        created: 'Notes created per day',
        tags: 'Notes per tag',
        share: 'Share of the tags',
      },
      from: 'Created from',
      to: 'Created until',
    },
  },
};

const PERIOD = { from: '2026-09-01' };

function answer(data: unknown): Envelope {
  return { ok: true, data, meta: { requestId: 'req-report' } };
}

function report(tags: { tag: string; notes: number }[], period: Record<string, unknown>): Envelope {
  return answer({ bands: { tags }, charts: { created: period, tags: period, share: period } });
}

const DAYS = answer({
  points: [
    { at: '2026-09-12T00:00:00Z', value: 1 },
    { at: '2026-09-13T00:00:00Z', value: 3 },
  ],
});
const BY_TAG = answer({
  points: [
    { label: 'home', value: 1 },
    { label: 'work', value: 3 },
  ],
});

async function render(client: KeelMockClient): Promise<ComponentFixture<KeelReportComponent>> {
  TestBed.configureTestingModule({
    imports: [KeelReportComponent],
    providers: [
      providePhoenixIcons(),
      provideRouter([]),
      { provide: KeelClient, useValue: client },
    ],
  });
  await TestBed.inject(KeelBoot).load();
  await TestBed.inject(KeelI18n).load('en-GB');
  const fixture = TestBed.createComponent(KeelReportComponent);
  fixture.componentRef.setInput('descriptor', REPORT);
  fixture.componentRef.setInput('params', {});
  await settled(fixture);
  return fixture;
}

async function settled(fixture: ComponentFixture<KeelReportComponent>): Promise<void> {
  fixture.detectChanges();
  await fixture.whenStable();
  fixture.detectChanges();
}

function cells(element: HTMLElement, band: string, column: string): string[] {
  return [
    ...element.querySelectorAll(
      `[data-band="${band}"] phx-list-table .body [role="row"] [data-column="${column}"]`,
    ),
  ].map((cell) => cell.textContent?.trim() ?? '');
}

function charts(fixture: ComponentFixture<KeelReportComponent>): PhxChartComponent[] {
  return fixture.debugElement
    .queryAll(By.directive(PhxChartComponent))
    .map((debug) => debug.componentInstance as PhxChartComponent);
}

describe('keel-report', () => {
  afterEach(() => TestBed.resetTestingModule());

  it('a_report_renders_its_bands_with_totals_and_its_charts', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en-GB', messages: MESSAGES },
      descriptors: [REPORT, SERIES, EXPORT],
      answers: {
        [REPORT_NAME]: report(
          [
            { tag: 'home', notes: 1 },
            { tag: 'work', notes: 3 },
          ],
          {},
        ),
        [SERIES_NAME]: [DAYS, BY_TAG, BY_TAG],
      },
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;

    expect(element.getAttribute('data-phase')).toBe('loaded');
    expect(client.callsOf(REPORT_NAME).map((call) => call.params)).toEqual([{}]);
    const band = element.querySelector('[data-band="tags"]');
    expect(band?.querySelector('.title')?.textContent?.trim()).toBe('Tags');
    expect(cells(element, 'tags', 'tag')).toEqual(['home', 'work']);
    expect(cells(element, 'tags', 'notes')).toEqual(['1', '3']);
    expect(band?.querySelector('[data-total="notes"]')?.textContent?.trim()).toBe('Notes: 4');
    expect(band?.querySelector('[data-action="export"]')?.textContent?.trim()).toBe('Export');

    expect(client.callsOf(SERIES_NAME).map((call) => call.params)).toEqual([
      { by: 'day' },
      { by: 'tag' },
      { by: 'tag' },
    ]);
    expect(
      [...element.querySelectorAll('[data-chart]')].map((chart) => [
        chart.getAttribute('data-chart'),
        chart.getAttribute('data-phase'),
        chart.querySelector('.title')?.textContent?.trim(),
      ]),
    ).toEqual([
      ['created', 'loaded', 'Notes created per day'],
      ['tags', 'loaded', 'Notes per tag'],
      ['share', 'loaded', 'Share of the tags'],
    ]);
    const [line, bar, pie] = charts(fixture);
    expect(line?.type()).toBe('line');
    expect(line?.labels()).toEqual(['12 Sept 2026', '13 Sept 2026']);
    expect(line?.series()).toEqual([
      { key: 'created', label: 'Notes created per day', values: [1, 3] },
    ]);
    expect(bar?.type()).toBe('bar');
    expect(bar?.labels()).toEqual(['home', 'work']);
    expect(pie?.type()).toBe('pie');
    expect(pie?.series()[0]?.values).toEqual([1, 3]);
  });

  it('the_parameters_form_refetches_the_report', async () => {
    const client = new KeelMockClient({
      boot: BOOT,
      catalogue: { lang: 'en-GB', messages: MESSAGES },
      descriptors: [REPORT, SERIES],
      answers: {
        [REPORT_NAME]: [
          report([{ tag: 'work', notes: 3 }], {}),
          report([{ tag: 'home', notes: 1 }], PERIOD),
        ],
        [SERIES_NAME]: [DAYS, BY_TAG, BY_TAG, DAYS, BY_TAG, BY_TAG],
      },
    });
    const fixture = await render(client);
    const element: HTMLElement = fixture.nativeElement;
    expect(cells(element, 'tags', 'tag')).toEqual(['work']);
    expect(
      [...element.querySelectorAll('keel-field')].map((field) =>
        field.querySelector('.label')?.textContent?.trim(),
      ),
    ).toEqual(['Created from', 'Created until']);

    fixture.componentInstance.form().group.controls['from']?.setValue(PERIOD.from);
    element.querySelector<HTMLButtonElement>('[data-action="apply"]')?.click();
    await settled(fixture);

    expect(client.callsOf(REPORT_NAME).map((call) => call.params)).toEqual([
      {},
      { from: '2026-09-01', to: null },
    ]);
    expect(cells(element, 'tags', 'tag')).toEqual(['home']);
    expect(client.callsOf(SERIES_NAME).map((call) => call.params)).toEqual([
      { by: 'day' },
      { by: 'tag' },
      { by: 'tag' },
      { by: 'day', from: '2026-09-01' },
      { by: 'tag', from: '2026-09-01' },
      { by: 'tag', from: '2026-09-01' },
    ]);
    expect(element.querySelector('[data-action="export"]')).toBeNull();
  });
});
