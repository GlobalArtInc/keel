import { provideHttpClient, withFetch } from '@angular/common/http';
import type { ApplicationRef, EnvironmentProviders, Provider } from '@angular/core';
import { bootstrapApplication } from '@angular/platform-browser';
import { provideAnimationsAsync } from '@angular/platform-browser/animations/async';
import { provideKeelClient } from '@globalart/keel/client';
import { provideKeelWidgets, type KeelWidget } from '@globalart/keel/forms';
import { KEEL_WORKSPACE } from '@globalart/keel/renderer';
import { providePhoenixIcons } from '@globalart/phoenix-ui';
import type { LucideIcons } from 'lucide-angular';
import { provideKeelLabels } from './labels';
import { provideKeelProducts, type KeelProduct } from './products';
import { provideKeelRouter } from './routes';
import { KeelShellComponent } from './shell';
import { KeelWorkspace } from './workspace/workspace';

export interface KeelShellOptions {
  products?: readonly KeelProduct[];
  icons?: LucideIcons;
  widgets?: readonly KeelWidget[];
}

export function keelProviders(options: KeelShellOptions = {}): (Provider | EnvironmentProviders)[] {
  return [
    provideKeelRouter(),
    provideHttpClient(withFetch()),
    provideAnimationsAsync(),
    providePhoenixIcons(options.icons ?? {}),
    provideKeelClient(),
    provideKeelWidgets(options.widgets ?? []),
    provideKeelProducts(options.products ?? []),
    provideKeelLabels(),
    { provide: KEEL_WORKSPACE, useExisting: KeelWorkspace },
  ];
}

export function bootstrapKeel(options: KeelShellOptions = {}): Promise<ApplicationRef> {
  return bootstrapApplication(KeelShellComponent, { providers: keelProviders(options) });
}
