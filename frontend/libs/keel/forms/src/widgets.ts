import {
  InjectionToken,
  inject,
  makeEnvironmentProviders,
  type EnvironmentProviders,
  type Type,
} from '@angular/core';

export interface KeelWidget {
  name: string;
  component: Type<unknown>;
}

export const KEEL_WIDGETS = new InjectionToken<readonly (readonly KeelWidget[])[]>('KEEL_WIDGETS');

export function provideKeelWidgets(widgets: readonly KeelWidget[]): EnvironmentProviders {
  const names = new Set<string>();
  for (const widget of widgets) {
    if (names.has(widget.name)) {
      throw new RangeError(`the widget ${widget.name} is registered twice`);
    }
    names.add(widget.name);
  }
  return makeEnvironmentProviders([{ provide: KEEL_WIDGETS, useValue: widgets, multi: true }]);
}

export function injectKeelWidgets(): readonly KeelWidget[] {
  return (inject(KEEL_WIDGETS, { optional: true }) ?? []).flat();
}

export function widgetOf(widgets: readonly KeelWidget[], name: string): KeelWidget | undefined {
  return widgets.find((widget) => widget.name === name);
}
