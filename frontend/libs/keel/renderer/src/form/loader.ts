import { signal, type Signal } from '@angular/core';
import { KeelCallError } from '@globalart/keel/client';
import { loadForm, unreachable, type KeelForm, type LoadOptions } from '@globalart/keel/forms';
import type { CallError, Descriptor } from '@globalart/keel/meta';

export type FormScreenState =
  { phase: 'loading' } | { phase: 'ready'; form: KeelForm } | { phase: 'failed'; error: CallError };

export type FormLoadOptions = Omit<LoadOptions, 'params'>;

export interface FormLoader {
  readonly state: Signal<FormScreenState>;
  reload(descriptor: Descriptor, params: Record<string, unknown>): void;
  drop(): void;
}

export function formLoader(options: () => FormLoadOptions): FormLoader {
  const held = signal<FormScreenState>({ phase: 'loading' });
  let sequence = 0;
  const drop = (): void => {
    const state = held();
    if (state.phase === 'ready') {
      state.form.destroy();
    }
  };
  return {
    state: held.asReadonly(),
    drop,
    reload(descriptor, params) {
      const started = ++sequence;
      drop();
      held.set({ phase: 'loading' });
      loadForm(descriptor, { ...options(), params }).then(
        (form) => {
          if (started !== sequence) {
            form.destroy();
            return;
          }
          held.set({ phase: 'ready', form });
        },
        (error: unknown) => {
          if (started !== sequence) {
            return;
          }
          held.set({
            phase: 'failed',
            error: error instanceof KeelCallError ? error.error : unreachable(error),
          });
        },
      );
    },
  };
}
