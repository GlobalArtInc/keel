import type { Descriptor } from '@globalart/keel/meta';
import { substitute } from '../action/runner';
import type { ScreenTarget } from '../navigation';

export const SELECTED_PARAM = 'selected';

export function selectionOf(params: Record<string, unknown>): string | null {
  const selected = params[SELECTED_PARAM];
  return typeof selected === 'string' && selected !== '' ? selected : null;
}

export function withoutSelection(params: Record<string, unknown>): Record<string, unknown> {
  const rest: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(params)) {
    if (name !== SELECTED_PARAM) {
      rest[name] = value;
    }
  }
  return rest;
}

export function sameParams(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function detailOf(descriptor: Descriptor, selected: string | null): ScreenTarget | null {
  const ui = descriptor.ui;
  const detail = ui?.layout === 'master-detail' ? (ui.detail ?? null) : null;
  if (detail === null || selected === null) {
    return null;
  }
  return { screen: detail.screen, params: substitute(detail.params ?? {}, selected) };
}
