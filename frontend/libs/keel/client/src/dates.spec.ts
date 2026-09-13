import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { KeelBoot, KeelDates, provideKeelClient } from '@globalart/keel/client';
import type { Boot } from '@globalart/keel/meta';

const FIXTURES = resolve(__dirname, '../../../../../contracts/fixtures');
const BOOT = JSON.parse(readFileSync(join(FIXTURES, 'boot.json'), 'utf8')) as Boot;
const INSTANT = '2026-09-13T20:00:00Z';

async function bootWith(tz: string | null, lang = 'en'): Promise<KeelDates> {
  TestBed.configureTestingModule({
    providers: [provideHttpClient(), provideHttpClientTesting(), provideKeelClient()],
  });
  const http = TestBed.inject(HttpTestingController);
  const loading = TestBed.inject(KeelBoot).load();
  const principal = BOOT.principal ?? null;
  http.expectOne('/api/v1/boot').flush({
    ...BOOT,
    principal: principal === null ? null : { ...principal, tz, lang },
  });
  await loading;
  http.verify();
  return TestBed.inject(KeelDates);
}

describe('KeelDates', () => {
  it('formats the wire instant in the principal zone and answers the day of a filter there', async () => {
    const tokyo = await bootWith('Asia/Tokyo');
    expect(tokyo.zone()).toBe('Asia/Tokyo');
    expect(tokyo.format(INSTANT)).toBe('Sep 14, 2026, 5:00 AM');
    expect(tokyo.dayFilter(INSTANT)).toBe('2026-09-14');
    expect(tokyo.dayFilter(tokyo.parse(INSTANT))).toBe('2026-09-14');
    expect(tokyo.parse(INSTANT).toISOString()).toBe('2026-09-13T20:00:00.000Z');

    TestBed.resetTestingModule();
    const utc = await bootWith(null);
    expect(utc.zone()).toBe('UTC');
    expect(utc.format(INSTANT)).toBe('Sep 13, 2026, 8:00 PM');
    expect(utc.dayFilter(INSTANT)).toBe('2026-09-13');
  });

  it('formats in the principal language and falls back to UTC on an unknown zone', async () => {
    const british = await bootWith('Europe/Belgrade', 'en-GB');
    expect(british.locale()).toBe('en-GB');
    expect(british.format(INSTANT, { dateStyle: 'long' })).toBe('13 September 2026');
    expect(british.dayFilter(INSTANT)).toBe('2026-09-13');

    TestBed.resetTestingModule();
    const unknown = await bootWith('Mars/Olympus');
    expect(unknown.dayFilter(INSTANT)).toBe('2026-09-13');
    expect(unknown.format(INSTANT)).toBe('Sep 13, 2026, 8:00 PM');
  });
});
