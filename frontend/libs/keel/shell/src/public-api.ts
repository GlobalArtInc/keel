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
export { BOOT_FAILED_KEY, KeelShellComponent } from './shell';
export {
  ACTING_AS_KEY,
  ACTING_EXIT_KEY,
  KeelLayoutComponent,
  MENU_KEY,
  SIDEBAR_STORAGE_KEY,
  plainClick,
} from './layout/layout.component';
export {
  PLATFORM_LEVEL_PREFIX,
  PRODUCT_PREFIX,
  brandOf,
  levelKey,
  levelLabel,
  menuEntries,
  platformGroups,
  productGroups,
  sidebarOf,
} from './layout/sidebar';
export type {
  BrandView,
  MenuEntry,
  SidebarGroup,
  SidebarSection,
  Translate,
} from './layout/sidebar';
export { BASE_CHROMA, applyBrand, chromaScale } from './layout/theme';
export {
  ACT_AS_ENTRY,
  KeelRailComponent,
  LANGUAGES,
  LANGUAGE_PREFIX,
  LANG_PREF,
  LOGOUT_ENTRY,
  PREF_SET,
  RAIL_HELP_KEY,
  RAIL_LATER_KEY,
  RAIL_NOTIFICATIONS_KEY,
  RAIL_TASKS_KEY,
  SECURITY_FUNCTION,
  SESSIONS_FUNCTION,
  THEME_PREF,
  TOKENS_FUNCTION,
  USER_ACT_AS_KEY,
  USER_LANGUAGE_KEY,
  USER_LOGOUT_KEY,
  USER_SCREENS,
  USER_SECURITY_KEY,
  USER_SESSIONS_KEY,
  USER_THEME_KEY,
  USER_TOKENS_KEY,
  languageOptions,
  userMenuItems,
} from './rail/rail.component';
export {
  ACT_AS_EMPTY_KEY,
  ACT_AS_ENTER_KEY,
  ACT_AS_PAGE_SIZE,
  ACT_AS_SEARCH_KEY,
  ACT_AS_TITLE_KEY,
  KeelActAsComponent,
  PRINCIPAL_LIST,
  principalQuery,
} from './rail/act-as.component';
export type { ActAsState, PrincipalRow } from './rail/act-as.component';
export { KeelSu } from './rail/su';
export {
  KeelNotFoundComponent,
  NOT_FOUND_HOME_KEY,
  NOT_FOUND_TEXT_KEY,
  NOT_FOUND_TITLE_KEY,
  pathOf,
} from './not-found';

export const KEEL_SHELL_VERSION = '0.1.0';
