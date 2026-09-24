import type { EnvironmentProviders } from '@angular/core';
import { provideKeelViews, type KeelView } from '@globalart/keel/renderer';

export const DNS_ID = 'dns';

export const DNS_VIEWS: readonly KeelView[] = [];

export function provideKeelDns(): EnvironmentProviders {
  return provideKeelViews(DNS_VIEWS);
}
