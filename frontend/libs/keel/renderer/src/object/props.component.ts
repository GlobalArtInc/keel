import { ChangeDetectionStrategy, Component, computed, inject, input } from '@angular/core';
import { KeelDates, KeelI18n } from '@globalart/keel/client';
import { lookup, type Descriptor } from '@globalart/keel/meta';
import { PhxPropComponent, PhxPropListComponent } from '@globalart/phoenix-ui';
import { EMPTY_VALUE, FALSE_KEY, TRUE_KEY } from '../messages';
import { objectRows, type ObjectRow, type ObjectValue } from './rows';

export interface PropRow {
  path: string;
  label: string;
  value: string;
  mono: boolean;
}

@Component({
  selector: 'keel-props',
  imports: [PhxPropListComponent, PhxPropComponent],
  template: `
    <phx-prop-list>
      @for (row of props(); track row.path) {
        <phx-prop
          [label]="row.label"
          [value]="row.value"
          [mono]="row.mono"
          [attr.data-path]="row.path"
        />
      }
    </phx-prop-list>
  `,
  styles: `
    :host {
      display: block;
    }
  `,
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class KeelPropsComponent {
  readonly descriptor = input.required<Descriptor>();
  readonly data = input.required<unknown>();

  private readonly i18n = inject(KeelI18n);
  private readonly dates = inject(KeelDates);

  readonly rows = computed(() => objectRows(this.descriptor(), this.data()));
  readonly props = computed<PropRow[]>(() =>
    this.rows().map((row) => ({
      path: row.path,
      label: this.labelOf(row),
      value: this.valueOf(row.value),
      mono: row.value.kind === 'json',
    })),
  );

  private labelOf(row: ObjectRow): string {
    const messages = this.i18n.messages();
    return lookup(messages, row.labelKey) ?? lookup(messages, row.columnKey) ?? row.labelKey;
  }

  private valueOf(value: ObjectValue): string {
    switch (value.kind) {
      case 'empty':
        return EMPTY_VALUE;
      case 'text':
      case 'json':
        return value.text;
      case 'enum':
        return lookup(this.i18n.messages(), value.labelKey) ?? value.value;
      case 'boolean':
        return this.i18n.t(value.value ? TRUE_KEY : FALSE_KEY);
      case 'datetime':
        return this.dates.format(value.value);
    }
  }
}
