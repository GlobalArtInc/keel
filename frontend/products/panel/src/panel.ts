import type { EnvironmentProviders } from '@angular/core';
import { provideKeelViews, type KeelView } from '@globalart/keel/renderer';

export const PANEL_ID = 'panel';

export const PANEL_BRAND = 'KeelPanel';

export const PANEL_ICON = 'server';

export const PANEL_VIEWS: readonly KeelView[] = [];

export function provideKeelPanel(): EnvironmentProviders {
  return provideKeelViews(PANEL_VIEWS);
}
