import { Injectable, inject, signal } from '@angular/core';
import { Router } from '@angular/router';
import { AUTH_SU, KeelAuth, KeelDescriptors, type Delegation } from '@globalart/keel/client';
import type { Envelope, Id } from '@globalart/keel/meta';
import { HOME_URL } from '../guard';

@Injectable({ providedIn: 'root' })
export class KeelSu {
  private readonly auth = inject(KeelAuth);
  private readonly descriptors = inject(KeelDescriptors);
  private readonly router = inject(Router);
  private readonly offered = signal(false);
  private probing = 0;

  readonly available = this.offered.asReadonly();

  async probe(): Promise<boolean> {
    const attempt = ++this.probing;
    this.offered.set(false);
    const offered = await this.descriptors
      .list()
      .then((index) => index.some((descriptor) => descriptor.name === AUTH_SU))
      .catch(() => false);
    if (attempt === this.probing) {
      this.offered.set(offered);
    }
    return offered;
  }

  enter(principal: Id): Promise<Envelope<Delegation>> {
    return this.settle(this.auth.su({ principal }));
  }

  exit(): Promise<Envelope<Delegation>> {
    return this.settle(this.auth.suExit());
  }

  private async settle(call: Promise<Envelope<Delegation>>): Promise<Envelope<Delegation>> {
    const envelope = await call;
    if (envelope.ok) {
      await this.router.navigateByUrl(HOME_URL);
    }
    return envelope;
  }
}
