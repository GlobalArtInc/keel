import { KEEL_META_VERSION } from '@globalart/keel/meta';
import { version } from '../../package.json';

describe('@globalart/keel/meta', () => {
  it('carries the version of the package', () => {
    expect(KEEL_META_VERSION).toBe(version);
  });
});
