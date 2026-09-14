import { Injectable, computed, inject, signal } from '@angular/core';
import type { Boot } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelClient, type Fetched } from './client';

@Injectable({ providedIn: 'root' })
export class KeelBoot {
  private readonly client = inject(KeelClient);
  private readonly held = signal<Fetched<Boot> | null>(null);

  readonly payload = computed(() => this.held()?.body ?? null);
  readonly loaded = computed(() => this.held() !== null);
  readonly principal = computed(() => this.payload()?.principal ?? null);
  readonly products = computed(() => this.payload()?.products ?? []);
  readonly brand = computed(() => this.payload()?.brand ?? null);
  readonly menu = computed(() => this.payload()?.menu ?? []);
  readonly views = computed(() => this.payload()?.views ?? []);
  readonly features = computed(() => this.payload()?.features ?? []);
  readonly meta = computed(() => this.payload()?.meta ?? null);

  load(): Promise<Boot> {
    return this.fetch(this.held()?.etag ?? null);
  }

  refresh(): Promise<Boot> {
    return this.fetch(null);
  }

  drop(): void {
    this.held.set(null);
  }

  private async fetch(etag: string | null): Promise<Boot> {
    const held = this.held();
    const fetched = await firstValueFrom(this.client.boot(etag));
    const next = fetched ?? held;
    if (next === null) {
      throw new TypeError('the boot answered not modified before any payload was held');
    }
    this.held.set(next);
    return next.body;
  }
}
