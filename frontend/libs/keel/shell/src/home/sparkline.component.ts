import { ChangeDetectionStrategy, Component, computed, input } from '@angular/core';

const WIDTH = 100;
const HEIGHT = 32;

function rounded(value: number): number {
  return Math.round(value * 100) / 100;
}

export function polyline(points: readonly number[]): string {
  if (points.length === 0) {
    return '';
  }
  const low = Math.min(...points);
  const span = Math.max(...points) - low;
  const step = points.length > 1 ? WIDTH / (points.length - 1) : 0;
  return points
    .map((value, index) => {
      const y = span === 0 ? HEIGHT / 2 : HEIGHT - ((value - low) / span) * HEIGHT;
      return `${rounded(index * step)},${rounded(y)}`;
    })
    .join(' ');
}

@Component({
  selector: 'keel-sparkline',
  template: `
    <svg
      class="svg"
      [attr.viewBox]="viewBox"
      preserveAspectRatio="none"
      role="img"
      [attr.aria-label]="label()"
    >
      <polyline class="line" [attr.points]="path()" vector-effect="non-scaling-stroke" />
    </svg>
  `,
  styles: `
    :host {
      display: block;
      height: 32px;
    }
    .svg {
      display: block;
      width: 100%;
      height: 100%;
      overflow: visible;
    }
    .line {
      fill: none;
      stroke: var(--phx-accent);
      stroke-width: 1.5;
      stroke-linecap: round;
      stroke-linejoin: round;
    }
  `,
  host: {
    '[attr.data-points]': 'points().length',
  },
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelSparklineComponent {
  readonly points = input.required<readonly number[]>();
  readonly label = input('');

  protected readonly viewBox = `0 0 ${WIDTH} ${HEIGHT}`;
  protected readonly path = computed(() => polyline(this.points()));
}
