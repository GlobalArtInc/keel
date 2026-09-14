import { Location } from '@angular/common';
import { Component, computed, effect, inject, untracked } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { Router, RouterOutlet } from '@angular/router';
import { KeelBoot, KeelClient, KeelI18n } from '@globalart/keel/client';
import {
  PHX_LABELS,
  PhxErrorComponent,
  PhxFullscreenLayoutComponent,
  PhxLoaderComponent,
  PhxThemeService,
} from '@globalart/phoenix-ui';
import { HOME_URL, KeelStartup, authRoute, loginUrl } from './guard';
import { applyBrand } from './layout/theme';

export const BOOT_FAILED_KEY = 'keel.shell.boot_failed';

@Component({
  selector: 'keel-shell',
  imports: [RouterOutlet, PhxFullscreenLayoutComponent, PhxLoaderComponent, PhxErrorComponent],
  template: `
    @switch (startup.state()) {
      @case ('ready') {
        <router-outlet />
      }
      @case ('failed') {
        <phx-fullscreen-layout>
          <phx-error [text]="failedText()" [retryLabel]="labels.retry" (retry)="retry()" />
        </phx-fullscreen-layout>
      }
      @default {
        <phx-fullscreen-layout>
          <div class="loading" role="status">
            <phx-loader [size]="24" [label]="labels.loading" />
          </div>
        </phx-fullscreen-layout>
      }
    }
  `,
  styles: `
    :host {
      display: block;
      min-height: 100%;
    }
    .loading {
      display: flex;
      justify-content: center;
      padding: 24px 0 40px;
    }
    phx-error {
      margin-bottom: 40px;
    }
  `,
  host: {
    '[attr.data-startup]': 'startup.state()',
  },
})
export class KeelShellComponent {
  protected readonly startup = inject(KeelStartup);
  protected readonly labels = inject(PHX_LABELS);
  private readonly client = inject(KeelClient);
  private readonly boot = inject(KeelBoot);
  private readonly i18n = inject(KeelI18n);
  private readonly theme = inject(PhxThemeService);
  private readonly router = inject(Router);
  private readonly location = inject(Location);

  protected readonly failedText = computed(() => this.i18n.t(BOOT_FAILED_KEY));

  constructor() {
    this.client.unauthenticated.pipe(takeUntilDestroyed()).subscribe(() => this.expire());
    effect(() => {
      const brand = this.boot.brand();
      if (brand !== null) {
        untracked(() => applyBrand(this.theme, brand));
      }
    });
  }

  retry(): void {
    this.startup.reset();
    void this.router
      .navigateByUrl(this.location.path() || HOME_URL, { onSameUrlNavigation: 'reload' })
      .catch(() => false);
  }

  private expire(): void {
    const current = this.router.url;
    if (authRoute(current)) {
      return;
    }
    this.startup.reset();
    void this.router.navigateByUrl(loginUrl(this.router, current));
  }
}
