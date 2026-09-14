import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { ActivatedRoute, Router, type UrlSegment } from '@angular/router';
import { KeelI18n } from '@globalart/keel/client';
import { PhxNotFoundComponent } from '@globalart/phoenix-ui';
import { map } from 'rxjs';
import { HOME_URL } from './guard';

export const NOT_FOUND_TITLE_KEY = 'keel.shell.not_found.title';
export const NOT_FOUND_TEXT_KEY = 'keel.shell.not_found.text';
export const NOT_FOUND_HOME_KEY = 'keel.shell.not_found.home';

export function pathOf(segments: readonly UrlSegment[]): string {
  return `/${segments.map((segment) => segment.path).join('/')}`;
}

@Component({
  selector: 'keel-not-found',
  imports: [PhxNotFoundComponent],
  template: `
    <phx-not-found
      [title]="title()"
      [text]="text()"
      [actionLabel]="homeLabel()"
      (action)="home()"
    />
  `,
  styles: `
    :host {
      display: block;
      height: 100%;
    }
  `,
  host: {
    '[attr.data-path]': 'path()',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelNotFoundComponent {
  private readonly i18n = inject(KeelI18n);
  private readonly router = inject(Router);
  private readonly route = inject(ActivatedRoute);

  readonly path = toSignal(this.route.url.pipe(map(pathOf)), { initialValue: HOME_URL });

  protected readonly title = computed(() => this.i18n.t(NOT_FOUND_TITLE_KEY));
  protected readonly text = computed(() => this.i18n.t(NOT_FOUND_TEXT_KEY, { path: this.path() }));
  protected readonly homeLabel = computed(() => this.i18n.t(NOT_FOUND_HOME_KEY));

  home(): void {
    void this.router.navigateByUrl(HOME_URL);
  }
}
