import { makeEnvironmentProviders, type EnvironmentProviders } from '@angular/core';
import { KeelClient, KeelHttpClient } from './client';

export function provideKeelClient(): EnvironmentProviders {
  return makeEnvironmentProviders([{ provide: KeelClient, useExisting: KeelHttpClient }]);
}
