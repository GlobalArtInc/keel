import {
  Injectable,
  InjectionToken,
  inject,
  type EnvironmentProviders,
  type Provider,
} from '@angular/core';
import type { Boot } from '@globalart/keel/meta';

export interface KeelProduct {
  id: string;
  load: () => Promise<EnvironmentProviders>;
}

export const KEEL_PRODUCTS = new InjectionToken<readonly KeelProduct[]>('KEEL_PRODUCTS');

export function provideKeelProducts(products: readonly KeelProduct[]): Provider {
  const ids = new Set<string>();
  for (const product of products) {
    if (ids.has(product.id)) {
      throw new RangeError(`the product ${product.id} is registered twice`);
    }
    ids.add(product.id);
  }
  return { provide: KEEL_PRODUCTS, useValue: products };
}

@Injectable({ providedIn: 'root' })
export class KeelProducts {
  private readonly registered = inject(KEEL_PRODUCTS, { optional: true }) ?? [];
  private readonly chunks = new Map<string, Promise<EnvironmentProviders>>();

  load(boot: Boot): Promise<EnvironmentProviders[]> {
    const enabled = new Set(boot.products.map((product) => product.id));
    return Promise.all(
      this.registered
        .filter((product) => enabled.has(product.id))
        .map((product) => this.chunk(product)),
    );
  }

  private chunk(product: KeelProduct): Promise<EnvironmentProviders> {
    const held = this.chunks.get(product.id);
    if (held !== undefined) {
      return held;
    }
    const pending = product.load().catch((error: unknown) => {
      this.chunks.delete(product.id);
      throw error;
    });
    this.chunks.set(product.id, pending);
    return pending;
  }
}
