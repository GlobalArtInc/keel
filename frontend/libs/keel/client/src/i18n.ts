import { Injectable, computed, inject, signal } from '@angular/core';
import { format, lookup, type Messages } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelClient, type Catalogue, type Fetched } from './client';

const NO_MESSAGES: Messages = {};

@Injectable({ providedIn: 'root' })
export class KeelI18n {
  private readonly client = inject(KeelClient);
  private readonly held = signal<Fetched<Catalogue> | null>(null);
  private requested: string | null = null;

  readonly lang = computed(() => this.held()?.body.lang ?? null);
  readonly messages = computed<Messages>(() => this.held()?.body.messages ?? NO_MESSAGES);

  async load(lang: string): Promise<Catalogue> {
    const held = this.held();
    const etag = held !== null && this.requested === lang ? held.etag : null;
    const fetched = await firstValueFrom(this.client.catalogue(lang, etag));
    const next = fetched ?? held;
    if (next === null) {
      throw new TypeError('the catalogue answered not modified before any was held');
    }
    this.requested = lang;
    this.held.set(next);
    return next.body;
  }

  t(key: string, params?: Record<string, unknown>): string {
    const message = lookup(this.messages(), key);
    return message === undefined ? key : format(message, params);
  }

  has(key: string): boolean {
    return lookup(this.messages(), key) !== undefined;
  }
}
