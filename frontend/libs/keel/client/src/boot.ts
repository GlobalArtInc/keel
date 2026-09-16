import { Injectable, computed, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import type { Boot } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelClient, type Fetched } from './client';

@Injectable({ providedIn: 'root' })
export class KeelBoot {
  private readonly client = inject(KeelClient);
  private readonly held = signal<Fetched<Boot> | null>(null);
  private pending: Promise<Boot> | null = null;

  readonly payload = computed(() => this.held()?.body ?? null);
  readonly loaded = computed(() => this.held() !== null);
  readonly principal = computed(() => this.payload()?.principal ?? null);
  readonly products = computed(() => this.payload()?.products ?? []);
  readonly brand = computed(() => this.payload()?.brand ?? null);
  readonly menu = computed(() => this.payload()?.menu ?? []);
  readonly views = computed(() => this.payload()?.views ?? []);
  readonly features = computed(() => this.payload()?.features ?? []);
  readonly meta = computed(() => this.payload()?.meta ?? null);

  constructor() {
    this.client.metaChanged.pipe(takeUntilDestroyed()).subscribe(() => this.revalidate());
  }

  load(): Promise<Boot> {
    return this.pending ?? this.fetch(this.held()?.etag ?? null);
  }

  refresh(): Promise<Boot> {
    return this.fetch(null);
  }

  drop(): void {
    this.pending = null;
    this.held.set(null);
  }

  private revalidate(): void {
    if (this.loaded()) {
      void this.load().catch(() => undefined);
    }
  }

  private fetch(etag: string | null): Promise<Boot> {
    const pending = firstValueFrom(this.client.boot(etag)).then((fetched) => {
      const next = fetched ?? this.held();
      if (next === null) {
        throw new TypeError('the boot answered not modified before any payload was held');
      }
      if (this.pending === pending) {
        this.held.set(next);
      }
      return next.body;
    });
    this.pending = pending;
    pending.then(
      () => this.settle(pending),
      () => this.settle(pending),
    );
    return pending;
  }

  private settle(pending: Promise<Boot>): void {
    if (this.pending === pending) {
      this.pending = null;
    }
  }
}
