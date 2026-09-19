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
  UNAUTHENTICATED_KEY,
  callHeaders,
  isEnvelope,
  requestIdOf,
  sessionExpired,
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
  AFTER_PARAM,
  DATA_INVALIDATE,
  KEEL_EVENTS_PATH,
  KEEL_EVENT_SOURCE,
  KeelEvents,
  MAX_BACKOFF_MS,
  META_INVALIDATE,
  MIN_BACKOFF_MS,
  NOTIFICATION,
  TASK_PROGRESS,
  isEvent,
  isMetaInvalidate,
  isNotificationFrame,
  isTaskProgress,
} from './events';
export type {
  EventSourceFactory,
  EventSourceLike,
  EventsPhase,
  EventsState,
  MetaInvalidate,
} from './events';
export { tracked } from './progress';
export { RAW_PREFIX, downloadUrl, filenameOf } from './download';
export type { DownloadResult, DownloadedFile } from './download';
export {
  ACTIVE_STATES,
  TASK_CANCEL,
  TASK_GET,
  TASK_LIST,
  isActive,
  isDownloadRef,
  isTaskRef,
  recentTasks,
} from './tasks';
export type { TaskDetail, TaskRow } from './tasks';
export {
  KeelNotifications,
  MESSAGE_CLEAR,
  MESSAGE_LIST,
  MESSAGE_MARK,
  mayCall,
  noteOf,
  problemKeyOf,
  recentMessages,
} from './notifications';
export type { Note } from './notifications';
export { KeelProblems, PROBLEM_LIST, countsByKey, openProblems } from './problems';
export type { Problem, Severity } from './problems';
export { provideKeelClient } from './provide';

export const KEEL_CLIENT_VERSION = '0.1.0';
