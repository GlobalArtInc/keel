import { ChangeDetectionStrategy, Component } from '@angular/core';
import { KEEL_SHELL_VERSION } from '@globalart/keel/shell';
import { PhxFullscreenLayoutComponent } from '@globalart/phoenix-ui';

@Component({
  selector: 'keel-root',
  imports: [PhxFullscreenLayoutComponent],
  template: `
    <phx-fullscreen-layout>
      <h1 class="name">Keel</h1>
      <p class="version">{{ version }}</p>
    </phx-fullscreen-layout>
  `,
  styles: `
    :host {
      display: block;
    }
    .name {
      margin: 0 0 8px;
      font: var(--phx-heading-25);
      text-align: center;
    }
    .version {
      margin: 0 0 40px;
      color: var(--phx-ink-muted);
      text-align: center;
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AppComponent {
  protected readonly version = KEEL_SHELL_VERSION;
}
