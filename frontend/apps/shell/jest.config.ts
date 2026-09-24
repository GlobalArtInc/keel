import type { Config } from 'jest';

const config: Config = {
  displayName: 'shell',
  preset: '../../jest.preset.js',
  setupFilesAfterEnv: ['<rootDir>/src/test-setup.ts'],
  moduleNameMapper: {
    '^@globalart/keel$': '<rootDir>/../../libs/keel/src/public-api.ts',
    '^@globalart/keel/(.*)$': '<rootDir>/../../libs/keel/$1/src/public-api.ts',
    '^@globalart/keel-demo$': '<rootDir>/../../products/demo/src/public-api.ts',
    '^@globalart/keel-dns$': '<rootDir>/../../products/dns/src/public-api.ts',
  },
};

export default config;
