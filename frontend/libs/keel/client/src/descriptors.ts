import { Injectable, inject } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import type { Descriptor } from '@globalart/keel/meta';
import { firstValueFrom } from 'rxjs';
import { KeelClient } from './client';

@Injectable({ providedIn: 'root' })
export class KeelDescriptors {
  private readonly client = inject(KeelClient);
  private readonly entries = new Map<string, Promise<Descriptor>>();
  private index: Promise<Descriptor[]> | null = null;
  private version: string | null = null;

  constructor() {
    this.client.metaChanged.pipe(takeUntilDestroyed()).subscribe(() => this.drop());
  }

  get(func: string): Promise<Descriptor> {
    this.sync();
    const cached = this.entries.get(func);
    if (cached !== undefined) {
      return cached;
    }
    const pending = firstValueFrom(this.client.descriptor(func));
    this.entries.set(func, pending);
    pending.catch(() => {
      if (this.entries.get(func) === pending) {
        this.entries.delete(func);
      }
    });
    return pending;
  }

  list(): Promise<Descriptor[]> {
    this.sync();
    if (this.index !== null) {
      return this.index;
    }
    const pending = firstValueFrom(this.client.descriptors());
    this.index = pending;
    pending.then(
      (descriptors) => {
        if (this.index !== pending) {
          return;
        }
        for (const descriptor of descriptors) {
          this.entries.set(descriptor.name, Promise.resolve(descriptor));
        }
      },
      () => {
        if (this.index === pending) {
          this.index = null;
        }
      },
    );
    return pending;
  }

  drop(): void {
    this.entries.clear();
    this.index = null;
    this.version = this.client.metaVersion();
  }

  private sync(): void {
    if (this.client.metaVersion() !== this.version) {
      this.drop();
    }
  }
}
