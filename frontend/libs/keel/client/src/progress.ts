import type { PhxGlobalProgressService } from '@globalart/phoenix-ui';
import { defer, finalize, type MonoTypeOperatorFunction } from 'rxjs';

export function tracked<T>(progress: PhxGlobalProgressService): MonoTypeOperatorFunction<T> {
  return (source) =>
    defer(() => {
      const release = progress.start();
      return source.pipe(finalize(release));
    });
}
