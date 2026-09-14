import { isObject } from '@globalart/keel/forms';
import type { Descriptor, ScreenPage } from '@globalart/keel/meta';
import { create } from 'qrcode';

export const PASSWORD_SET = 'auth.password.set';
export const TOTP_ENROLL = 'auth.totp.enroll';
export const TOTP_CONFIRM = 'auth.totp.confirm';
export const TOTP_DISABLE = 'auth.totp.disable';
export const PASSWORD_FIELDS: readonly string[] = ['current', 'password'];
export const CODE_LABEL_KEY = 'auth.totp.code';
export const ENROLL_TITLE_KEY = 'auth.totp.enroll.title';
export const DISABLE_TITLE_KEY = 'auth.totp.disable.title';
export const SECURITY_TITLE_KEY = 'keel.shell.security.title';
export const SECURITY_PASSWORD_KEY = 'keel.shell.security.password';
export const SECURITY_PASSWORD_HINT_KEY = 'keel.shell.security.password_hint';
export const SECURITY_TOTP_KEY = 'keel.shell.security.totp';
export const SECURITY_ENROLLED_KEY = 'keel.shell.security.enrolled';
export const SECURITY_NOT_ENROLLED_KEY = 'keel.shell.security.not_enrolled';
export const SECURITY_ENROLL_KEY = 'keel.shell.security.enroll';
export const SECURITY_CONFIRM_KEY = 'keel.shell.security.confirm';
export const SECURITY_DISABLE_KEY = 'keel.shell.security.disable';
export const SECURITY_SECRET_KEY = 'keel.shell.security.secret';
export const SECURITY_SECRET_HINT_KEY = 'keel.shell.security.secret_hint';
export const SECURITY_RECOVERY_KEY = 'keel.shell.security.recovery';
export const SECURITY_RECOVERY_HINT_KEY = 'keel.shell.security.recovery_hint';
export const QUIET_ZONE = 4;

export interface Enrolled {
  secret: string;
  uri: string;
}

export interface QrImage {
  size: number;
  path: string;
}

export function isEnrolled(value: unknown): value is Enrolled {
  return isObject(value) && typeof value['secret'] === 'string' && typeof value['uri'] === 'string';
}

export function recoveryCodesOf(value: unknown): string[] {
  if (!isObject(value) || !Array.isArray(value['recoveryCodes'])) {
    return [];
  }
  return value['recoveryCodes'].filter((code): code is string => typeof code === 'string');
}

export function ownPasswordDescriptor(descriptor: Descriptor): Descriptor {
  const pages: ScreenPage[] = (descriptor.ui?.pages ?? []).map((page) => ({
    ...page,
    fields: (page.fields ?? []).filter((field) => PASSWORD_FIELDS.includes(field)),
  }));
  return {
    ...descriptor,
    ui: {
      ...(descriptor.ui ?? { title: descriptor.name }),
      pages: pages.length > 0 ? pages : [{ id: 'main', fields: [...PASSWORD_FIELDS] }],
    },
  };
}

export function qrOf(text: string): QrImage {
  const { modules } = create(text, { errorCorrectionLevel: 'M' });
  const parts: string[] = [];
  for (let row = 0; row < modules.size; row++) {
    for (let column = 0; column < modules.size; column++) {
      if (modules.get(row, column)) {
        parts.push(`M${column} ${row}h1v1h-1z`);
      }
    }
  }
  return { size: modules.size, path: parts.join('') };
}

export function viewBoxOf(image: QrImage): string {
  const edge = image.size + QUIET_ZONE * 2;
  return `${-QUIET_ZONE} ${-QUIET_ZONE} ${edge} ${edge}`;
}
