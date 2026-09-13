import { KEEL_CLIENT_VERSION } from '@globalart/keel/client';
import { version } from '../../package.json';

describe('@globalart/keel/client', () => {
  it('carries the version of the package', () => {
    expect(KEEL_CLIENT_VERSION).toBe(version);
  });
});
