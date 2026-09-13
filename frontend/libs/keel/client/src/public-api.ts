export {
  API_PREFIX,
  IDEMPOTENCY_KEY_HEADER,
  IDEMPOTENT_REPLAY_HEADER,
  KeelCallError,
  KeelClient,
  KeelHttpClient,
  LANG_PARAM,
  META_VERSION_HEADER,
  REQUEST_ID_HEADER,
  callHeaders,
  isEnvelope,
  requestIdOf,
} from './client';
export type { CallOptions, Catalogue, Fetched } from './client';
export { KeelBoot } from './boot';
export { KeelDescriptors } from './descriptors';
export { KeelI18n } from './i18n';
export {
  AUTH_KEY_REDEEM,
  AUTH_LOGIN,
  AUTH_LOGOUT,
  AUTH_SU,
  AUTH_SU_EXIT,
  AUTH_TOTP_VERIFY,
  KeelAuth,
} from './auth';
export type {
  Delegation,
  LoggedIn,
  LoggedOut,
  LoginParams,
  Redeemed,
  RedeemKeyParams,
  SuParams,
  VerifyTotpParams,
} from './auth';
export { listFetch, toListQuery } from './list';
export type { KeelListFetch, Scope, Totals } from './list';
export { DEFAULT_LOCALE, KeelDates, UTC } from './dates';
export {
  DATA_INVALIDATE,
  KEEL_EVENTS_PATH,
  KEEL_EVENT_SOURCE,
  KeelEvents,
  MAX_BACKOFF_MS,
  META_INVALIDATE,
  MIN_BACKOFF_MS,
  NOTIFICATION,
  TASK_PROGRESS,
} from './events';
export type { EventSourceFactory, EventSourceLike, EventsState } from './events';
export { tracked } from './progress';
export { provideKeelClient } from './provide';

export const KEEL_CLIENT_VERSION = '0.1.0';
