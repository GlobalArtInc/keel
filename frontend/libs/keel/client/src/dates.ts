import { Injectable, computed, inject } from '@angular/core';
import { KeelBoot } from './boot';

export const UTC = 'UTC';
export const DEFAULT_LOCALE = 'en';

const WIRE_LOCALE = 'en-US';
const DAY: Intl.DateTimeFormatOptions = { year: 'numeric', month: '2-digit', day: '2-digit' };
const DATETIME: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' };

function toDate(value: string | Date): Date {
  return value instanceof Date ? value : new Date(value);
}

@Injectable({ providedIn: 'root' })
export class KeelDates {
  private readonly boot = inject(KeelBoot);

  readonly zone = computed(() => this.boot.principal()?.tz ?? UTC);
  readonly locale = computed(() => this.boot.principal()?.lang ?? DEFAULT_LOCALE);

  parse(wire: string): Date {
    return new Date(wire);
  }

  format(value: string | Date, options: Intl.DateTimeFormatOptions = DATETIME): string {
    return this.formatter(this.locale(), options).format(toDate(value));
  }

  dayFilter(value: string | Date): string {
    const parts = this.formatter(WIRE_LOCALE, DAY).formatToParts(toDate(value));
    const part = (type: Intl.DateTimeFormatPartTypes): string =>
      parts.find((candidate) => candidate.type === type)?.value ?? '';
    return `${part('year')}-${part('month')}-${part('day')}`;
  }

  private formatter(locale: string, options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
    try {
      return new Intl.DateTimeFormat(locale, { ...options, timeZone: this.zone() });
    } catch {
      return new Intl.DateTimeFormat(DEFAULT_LOCALE, { ...options, timeZone: UTC });
    }
  }
}
