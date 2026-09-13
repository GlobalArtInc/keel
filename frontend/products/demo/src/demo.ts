import type { EnvironmentProviders } from '@angular/core';
import { provideKeelViews, type KeelView } from '@globalart/keel/renderer';
import { TAG_LIST } from './tag-cloud/cloud';
import { NoteTagCloudComponent } from './tag-cloud/tag-cloud.component';

export const DEMO_ID = 'demo';

export const DEMO_VIEWS: readonly KeelView[] = [
  { func: TAG_LIST, component: NoteTagCloudComponent },
];

export function provideKeelDemo(): EnvironmentProviders {
  return provideKeelViews(DEMO_VIEWS);
}
