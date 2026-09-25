import type { KeelI18n } from '@globalart/keel/client';
import type { CallError } from '@globalart/keel/meta';

export const RETRY_KEY = 'keel.action.retry';
export const APPLY_KEY = 'keel.action.apply';
export const EXPORT_KEY = 'keel.action.export';
export const REPORT_EMPTY_KEY = 'keel.report.empty';
export const DETAIL_EMPTY_KEY = 'keel.detail.empty';
export const STREAM_LIVE_KEY = 'keel.stream.live';
export const STREAM_ENDED_KEY = 'keel.stream.ended';
export const STREAM_FAILED_KEY = 'keel.stream.failed';
export const STREAM_RECONNECT_KEY = 'keel.stream.reconnect';
export const DONE_KEY = 'keel.action.done';
export const BACK_KEY = 'keel.action.back';
export const NEXT_KEY = 'keel.action.next';
export const FINISH_KEY = 'keel.action.finish';
export const REQUEST_KEY = 'keel.error.request';
export const NOT_FOUND_KEY = 'keel.error.not_found';
export const VIEW_MISSING_KEY = 'keel.view.missing';
export const SCREEN_UNSUPPORTED_KEY = 'keel.screen.unsupported';
export const CONFIRM_ONE_KEY = 'keel.confirm.one';
export const CONFIRM_MANY_KEY = 'keel.confirm.many';
export const CONFIRM_COUNT_KEY = 'keel.confirm.count';
export const TRUE_KEY = 'keel.value.true';
export const FALSE_KEY = 'keel.value.false';
export const EMPTY_VALUE = '—';

export function describeError(i18n: KeelI18n, error: CallError): string {
  const message = i18n.t(error.key, error.params);
  const id = error.requestId ?? null;
  return id === null ? message : i18n.t(REQUEST_KEY, { message, id });
}
