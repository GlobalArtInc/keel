import { provideLocationMocks } from '@angular/common/testing';
import {
  inject,
  provideAppInitializer,
  type ApplicationConfig,
  type EnvironmentProviders,
  type Provider,
} from '@angular/core';
import { provideRouter, withComponentInputBinding, type Routes } from '@angular/router';
import {
  KEEL_EVENT_SOURCE,
  KeelBoot,
  KeelClient,
  KeelI18n,
  type Catalogue,
  type EventSourceLike,
} from '@globalart/keel/client';
import { isObject } from '@globalart/keel/forms';
import type { Boot, Descriptor, Messages } from '@globalart/keel/meta';
import { GUEST_LANG, applyBrand, provideKeelLabels } from '@globalart/keel/shell';
import { PhxThemeService } from '@globalart/phoenix-ui';
import { catalogueOf, type DumpLevel, type KeelDump } from './dump';
import { KeelMockClient, type CannedAnswer } from './mock-client';

export const DEFAULT_LEVEL: DumpLevel = 'admin';

export interface WithKeelOptions {
  level?: DumpLevel;
  boot?: Boot;
  calls?: Record<string, CannedAnswer>;
  messages?: Messages;
  routes?: Routes;
}

export interface KeelStory {
  applicationConfig?: Partial<ApplicationConfig>;
}

export type KeelDecorator = <S extends KeelStory>(story: () => S) => S;

const registered = new Map<DumpLevel, KeelDump>();

export function registerKeelDumps(dumps: Iterable<KeelDump>): void {
  for (const dump of dumps) {
    registered.set(dump.level, dump);
  }
}

export function keelDump(level: DumpLevel): KeelDump {
  const dump = registered.get(level);
  if (dump === undefined) {
    throw new Error(`the dump of ${level} is not registered; call registerKeelDumps first`);
  }
  return dump;
}

export function descriptorOf(dump: KeelDump, func: string): Descriptor {
  const descriptor = dump.descriptors.find((entry) => entry.name === func);
  if (descriptor === undefined) {
    throw new Error(`the dump of ${dump.level} has no descriptor ${func}`);
  }
  return descriptor;
}

export function mergeMessages(base: Messages, extra: Messages): Messages {
  const merged: Messages = { ...base };
  for (const [key, value] of Object.entries(extra)) {
    const held = merged[key];
    merged[key] = isObject(held) && isObject(value) ? mergeMessages(held, value) : value;
  }
  return merged;
}

export class QuietEventSource implements EventSourceLike {
  readonly readyState = 0;

  addEventListener(): void {}

  close(): void {}
}

export function keelStoryClient(options: WithKeelOptions = {}): KeelMockClient {
  const dump = keelDump(options.level ?? DEFAULT_LEVEL);
  const own = catalogueOf(dump);
  const catalogue: Catalogue = {
    lang: own.lang,
    messages: mergeMessages(own.messages, options.messages ?? {}),
  };
  return new KeelMockClient({
    boot: options.boot ?? dump.boot,
    descriptors: dump.descriptors,
    catalogue,
    answers: options.calls ?? {},
  });
}

export function keelStoryProviders(
  options: WithKeelOptions = {},
): (Provider | EnvironmentProviders)[] {
  return [
    { provide: KeelClient, useValue: keelStoryClient(options) },
    { provide: KEEL_EVENT_SOURCE, useValue: () => new QuietEventSource() },
    provideRouter(
      [...(options.routes ?? []), { path: '**', children: [] }],
      withComponentInputBinding(),
    ),
    provideLocationMocks(),
    provideKeelLabels(),
    provideAppInitializer(async () => {
      const boot = inject(KeelBoot);
      const i18n = inject(KeelI18n);
      const theme = inject(PhxThemeService);
      const payload = await boot.load();
      await i18n.load(payload.principal?.lang ?? GUEST_LANG);
      applyBrand(theme, payload.brand);
    }),
  ];
}

export function withKeel(options: WithKeelOptions = {}): KeelDecorator {
  return (story) => {
    const rendered = story();
    return {
      ...rendered,
      applicationConfig: {
        ...rendered.applicationConfig,
        providers: [
          ...keelStoryProviders(options),
          ...(rendered.applicationConfig?.providers ?? []),
        ],
      },
    };
  };
}
