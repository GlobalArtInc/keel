export { bootstrapKeel, keelProviders } from './bootstrap';
export type { KeelShellOptions } from './bootstrap';
export { KEEL_PRODUCTS, KeelProducts, provideKeelProducts } from './products';
export type { KeelProduct } from './products';
export { FUNC_PARAM, keelRoutes, provideKeelRouter, screenRoutes } from './routes';
export {
  GUEST_LANG,
  HOME_URL,
  KeelStartup,
  LOGIN_PATH,
  LOGOUT_PATH,
  RETURN_TO,
  authRoute,
  firstSegment,
  keelGuard,
  keelHome,
  keelLogout,
  keelReady,
  loginUrl,
  returnTarget,
  returnable,
  screenUrl,
} from './guard';
export type { StartupState } from './guard';
export {
  BRAND_KEY,
  KEY_PARAM,
  KeelLoginComponent,
  LOGIN_CODE_HINT_KEY,
  LOGIN_CODE_KEY,
  LOGIN_FAILED_KEY,
  LOGIN_KEY_FAILED_KEY,
  LOGIN_KEY_KEY,
  LOGIN_LOGIN_KEY,
  LOGIN_PASSWORD_KEY,
  LOGIN_SUBMIT_KEY,
  LOGIN_TITLE_KEY,
  LOGIN_VERIFY_KEY,
} from './login/login.component';
export type { LoginNotice, LoginStep } from './login/login.component';
export {
  LABELS_PREFIX,
  LABEL_NAMES,
  bridgeLabels,
  isLabelName,
  labelOf,
  provideKeelLabels,
} from './labels';
export type { LabelName } from './labels';
export {
  BOOT_FAILED_KEY,
  KeelLayoutComponent,
  KeelShellComponent,
  MENU_KEY,
  menuEntries,
  plainClick,
} from './shell';
export type { MenuEntry } from './shell';
export {
  KeelNotFoundComponent,
  NOT_FOUND_HOME_KEY,
  NOT_FOUND_TEXT_KEY,
  NOT_FOUND_TITLE_KEY,
  pathOf,
} from './not-found';

export const KEEL_SHELL_VERSION = '0.1.0';
