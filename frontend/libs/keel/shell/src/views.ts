import type { EnvironmentProviders } from '@angular/core';
import { provideKeelViews, type KeelView } from '@globalart/keel/renderer';
import { KeelSecurityComponent } from './security/security.component';
import { TOTP_ENROLL } from './security/totp';
import { SETTINGS_SET } from './settings-value/setting';
import { KeelSettingsValueComponent } from './settings-value/settings-value.component';

export const SHELL_VIEWS: readonly KeelView[] = [
  { func: SETTINGS_SET, component: KeelSettingsValueComponent },
  { func: TOTP_ENROLL, component: KeelSecurityComponent },
];

export function provideKeelShellViews(): EnvironmentProviders {
  return provideKeelViews(SHELL_VIEWS);
}
