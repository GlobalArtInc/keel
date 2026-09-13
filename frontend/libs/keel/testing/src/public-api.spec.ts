import { KEEL_TESTING_VERSION } from '@globalart/keel/testing';
import { version } from '../../package.json';

describe('@globalart/keel/testing', () => {
  it('carries the version of the package', () => {
    expect(KEEL_TESTING_VERSION).toBe(version);
  });
});
