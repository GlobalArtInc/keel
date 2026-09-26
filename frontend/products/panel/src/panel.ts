import type { EnvironmentProviders } from '@angular/core';
import { provideKeelViews, type KeelView } from '@globalart/keel/renderer';
import { FileManagerComponent } from './file-manager/file-manager.component';
import { FILE_MANAGER } from './file-manager/files';

export const PANEL_ID = 'panel';

export const PANEL_BRAND = 'KeelPanel';

export const PANEL_ICON = 'server';

export const PANEL_VIEWS: readonly KeelView[] = [
  { func: FILE_MANAGER, component: FileManagerComponent },
];

export function provideKeelPanel(): EnvironmentProviders {
  return provideKeelViews(PANEL_VIEWS);
}
