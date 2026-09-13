import type { Config } from 'jest';

const config: Config = {
  displayName: 'keel',
  preset: '../../jest.preset.js',
  setupFilesAfterEnv: ['<rootDir>/src/test-setup.ts'],
  moduleNameMapper: {
    '^@globalart/keel$': '<rootDir>/src/public-api.ts',
    '^@globalart/keel/(.*)$': '<rootDir>/$1/src/public-api.ts',
  },
};

export default config;
