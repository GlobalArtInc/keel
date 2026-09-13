import type { Config } from 'jest';

const config: Config = {
  displayName: 'demo',
  preset: '../../jest.preset.js',
  setupFilesAfterEnv: ['<rootDir>/src/test-setup.ts'],
  moduleNameMapper: {
    '^@globalart/keel-demo$': '<rootDir>/src/public-api.ts',
    '^@globalart/keel$': '<rootDir>/../../libs/keel/src/public-api.ts',
    '^@globalart/keel/(.*)$': '<rootDir>/../../libs/keel/$1/src/public-api.ts',
  },
};

export default config;
