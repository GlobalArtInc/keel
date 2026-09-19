import {
  ChangeDetectionStrategy,
  Component,
  InjectionToken,
  computed,
  inject,
} from '@angular/core';
import { KeelI18n } from '@globalart/keel/client';
import type { Id } from '@globalart/keel/meta';
import { PhxProgressModalComponent, PhxTextButtonComponent } from '@globalart/phoenix-ui';

export const TASK_DETAILS_KEY = 'keel.shell.tasks.details';

export interface FollowedTask {
  id: Id;
  details: (() => void) | null;
}

export const KEEL_FOLLOWED_TASK = new InjectionToken<FollowedTask>('KEEL_FOLLOWED_TASK');

@Component({
  selector: 'keel-progress',
  imports: [PhxProgressModalComponent, PhxTextButtonComponent],
  template: `
    <phx-progress-modal />
    @if (followed.details !== null) {
      <div class="details">
        <button phxTextButton="link" type="button" class="open" (click)="followed.details()">
          {{ detailsLabel() }}
        </button>
      </div>
    }
  `,
  styles: `
    :host {
      display: block;
    }
    .details {
      display: flex;
      justify-content: flex-end;
      margin-top: 12px;
    }
  `,
  host: {
    '[attr.data-task]': 'followed.id',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelProgressComponent {
  protected readonly followed = inject(KEEL_FOLLOWED_TASK);
  private readonly i18n = inject(KeelI18n);

  protected readonly detailsLabel = computed(() => this.i18n.t(TASK_DETAILS_KEY));
}
