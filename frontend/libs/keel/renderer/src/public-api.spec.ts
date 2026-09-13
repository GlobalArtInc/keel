import { KEEL_RENDERER_VERSION } from '@globalart/keel/renderer';
import { version } from '../../package.json';

describe('@globalart/keel/renderer', () => {
  it('carries the version of the package', () => {
    expect(KEEL_RENDERER_VERSION).toBe(version);
  });
});
