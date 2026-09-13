import {
  InjectionToken,
  Injector,
  inject,
  makeEnvironmentProviders,
  type EnvironmentProviders,
  type Type,
} from '@angular/core';
import type { KeelClient } from '@globalart/keel/client';
import type { Boot, BootPrincipal, Descriptor } from '@globalart/keel/meta';
import type { PhxTabsService } from '@globalart/phoenix-ui';

export interface KeelView {
  func: string;
  component: Type<unknown>;
}

export interface KeelViewContext {
  descriptor?: Descriptor;
  params: Record<string, unknown>;
  client: KeelClient;
  principal: BootPrincipal | null;
  tabs: PhxTabsService;
}

export const KEEL_VIEWS = new InjectionToken<readonly (readonly KeelView[])[]>('KEEL_VIEWS');

export const KEEL_VIEW_CONTEXT = new InjectionToken<KeelViewContext>('KEEL_VIEW_CONTEXT');

export function provideKeelViews(views: readonly KeelView[]): EnvironmentProviders {
  const names = new Set<string>();
  for (const view of views) {
    if (names.has(view.func)) {
      throw new RangeError(`the view ${view.func} is registered twice`);
    }
    names.add(view.func);
  }
  return makeEnvironmentProviders([{ provide: KEEL_VIEWS, useValue: views, multi: true }]);
}

export function injectKeelViews(): readonly KeelView[] {
  return (inject(KEEL_VIEWS, { optional: true }) ?? []).flat();
}

export function injectKeelViewContext(): KeelViewContext {
  return inject(KEEL_VIEW_CONTEXT);
}

export function viewOf(views: readonly KeelView[], func: string): KeelView | undefined {
  return views.find((view) => view.func === func);
}

export function viewInjector(context: KeelViewContext, parent: Injector): Injector {
  return Injector.create({
    parent,
    providers: [{ provide: KEEL_VIEW_CONTEXT, useValue: context }],
  });
}

export function productOf(boot: Boot | null, func: string): string {
  for (const group of boot?.menu ?? []) {
    if (group.product && group.items.some((item) => item.func === func)) {
      return group.product;
    }
  }
  const dot = func.indexOf('.');
  return dot < 0 ? func : func.slice(0, dot);
}
