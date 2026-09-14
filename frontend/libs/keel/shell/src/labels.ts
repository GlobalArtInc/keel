import { inject, type Provider } from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import { lookup, type Messages } from '@globalart/keel/meta';
import { PHX_DEFAULT_LABELS, PHX_LABELS, type PhxLabels } from '@globalart/phoenix-ui';

export const LABELS_PREFIX = 'phx';

export type LabelName = keyof PhxLabels;

export const LABEL_NAMES = Object.keys(PHX_DEFAULT_LABELS) as readonly LabelName[];

export function isLabelName(name: PropertyKey): name is LabelName {
  return typeof name === 'string' && Object.hasOwn(PHX_DEFAULT_LABELS, name);
}

export function labelOf(messages: Messages, name: LabelName): string {
  return lookup(messages, `${LABELS_PREFIX}.${name}`) ?? PHX_DEFAULT_LABELS[name];
}

export function bridgeLabels(messages: () => Messages): PhxLabels {
  return new Proxy(PHX_DEFAULT_LABELS, {
    get(defaults, name) {
      return isLabelName(name) ? labelOf(messages(), name) : Reflect.get(defaults, name);
    },
  });
}

export function provideKeelLabels(): Provider {
  return { provide: PHX_LABELS, useFactory: () => bridgeLabels(inject(KeelI18n).messages) };
}
