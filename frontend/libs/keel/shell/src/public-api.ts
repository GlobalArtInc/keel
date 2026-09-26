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
  RESET_PATH,
  RETURN_TO,
  SETUP_PATH,
  VERIFY_PATH,
  authRoute,
  firstSegment,
  hasDashboard,
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
  LOGIN_FORGOT_KEY,
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
  KeelSetupComponent,
  SETUP_DONE_KEY,
  SETUP_FAILED_KEY,
  SETUP_INTRO_KEY,
  SETUP_LOGIN_KEY,
  SETUP_NO_KEY_KEY,
  SETUP_PASSWORD_KEY,
  SETUP_SIGNING_IN_KEY,
  SETUP_SUBMIT_KEY,
  SETUP_TITLE_KEY,
  SETUP_TO_LOGIN_KEY,
} from './login/setup.component';
export type { SetupNotice, SetupStage } from './login/setup.component';
export {
  KeelResetComponent,
  RESET_DONE_KEY,
  RESET_FAILED_KEY,
  RESET_INTRO_KEY,
  RESET_LOGIN_KEY,
  RESET_PASSWORD_KEY,
  RESET_SAVE_KEY,
  RESET_SENT_KEY,
  RESET_SUBMIT_KEY,
  RESET_TITLE_KEY,
  RESET_TO_LOGIN_KEY,
} from './login/reset.component';
export type { ResetNotice, ResetStage } from './login/reset.component';
export {
  KeelVerifyComponent,
  VERIFY_CONTINUE_KEY,
  VERIFY_DONE_KEY,
  VERIFY_FAILED_KEY,
  VERIFY_PENDING_KEY,
  VERIFY_TITLE_KEY,
} from './login/verify.component';
export type { VerifyState } from './login/verify.component';
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

export { SHELL_VIEWS, provideKeelShellViews } from './views';
export {
  KeelTaskWatch,
  TASKS_EMPTY_KEY,
  TASKS_ID_KEY,
  TASKS_LOG_KEY,
  TASKS_REQUEST_KEY,
  TASKS_TITLE_KEY,
  activeIds,
  functionLabel,
} from './tasks/tasks';
export { KeelTaskDrawerComponent, progressOf } from './tasks/task-drawer.component';
export type { DetailState, JobsState, TaskDrawerData } from './tasks/task-drawer.component';
export {
  KeelNotificationCentreComponent,
  NOTIFICATIONS_CLEAR_KEY,
  NOTIFICATIONS_DISMISS_KEY,
  NOTIFICATIONS_NO_PROBLEMS_KEY,
  NOTIFICATIONS_PROBLEMS_KEY,
  NOTIFICATIONS_SEEN_KEY,
  NOTIFICATIONS_TITLE_KEY,
  NOTIFICATIONS_UNREAD_KEY,
} from './notifications/notification-centre.component';
export type {
  NotificationCentreData,
  NotificationView,
} from './notifications/notification-centre.component';
export {
  ABOUT_API_KEY,
  ABOUT_EDITION_KEY,
  ABOUT_NO_PRODUCTS_KEY,
  ABOUT_PRODUCTS_KEY,
  ABOUT_TITLE_KEY,
  ABOUT_VERSION_KEY,
  KeelAboutComponent,
  productViews,
} from './about/about.component';
export type { ProductView } from './about/about.component';
export {
  DEFAULT_WIDGET_LIMIT,
  HOME_OPEN_KEY,
  HOME_TITLE_KEY,
  KeelHomeComponent,
  limitOf,
} from './home/home.component';
export type { StatReading, WidgetState } from './home/dashboard';
export {
  KEY_FIELD,
  SETTINGS_DEFAULT_KEY,
  SETTINGS_GET,
  SETTINGS_SET,
  SETTINGS_TITLE_KEY,
  VALUE_FIELD,
  isSettingRow,
  settingKeyOf,
  valueDescriptor,
  valueSchema,
  valuesOf,
} from './settings-value/setting';
export type { SettingRow } from './settings-value/setting';
export { KeelSettingsValueComponent } from './settings-value/settings-value.component';
export type { SettingState } from './settings-value/settings-value.component';
export {
  CODE_LABEL_KEY,
  DISABLE_TITLE_KEY,
  ENROLL_TITLE_KEY,
  PASSWORD_FIELDS,
  PASSWORD_SET,
  QUIET_ZONE,
  SECURITY_CONFIRM_KEY,
  SECURITY_DISABLE_KEY,
  SECURITY_ENROLLED_KEY,
  SECURITY_ENROLL_KEY,
  SECURITY_NOT_ENROLLED_KEY,
  SECURITY_PASSWORD_HINT_KEY,
  SECURITY_PASSWORD_KEY,
  SECURITY_RECOVERY_HINT_KEY,
  SECURITY_RECOVERY_KEY,
  SECURITY_SECRET_HINT_KEY,
  SECURITY_SECRET_KEY,
  SECURITY_TITLE_KEY,
  SECURITY_TOTP_KEY,
  TOTP_CONFIRM,
  TOTP_DISABLE,
  TOTP_ENROLL,
  isEnrolled,
  ownPasswordDescriptor,
  qrOf,
  recoveryCodesOf,
  viewBoxOf,
} from './security/totp';
export type { Enrolled, QrImage } from './security/totp';
export { CODE_FIELD, codeErrorOf, codeForm } from './security/code';
export type { CodeForm } from './security/code';
export {
  ADDRESS_FIELDS,
  ADDRESS_GET,
  ADDRESS_SET,
  KeelSecurityAddressComponent,
  SECURITY_ADDRESS_KEY,
  SECURITY_ADDRESS_NONE_KEY,
  SECURITY_ADDRESS_PENDING_KEY,
  SECURITY_ADDRESS_SENT_KEY,
  SECURITY_ADDRESS_VERIFIED_KEY,
  ownAddressDescriptor,
} from './security/address.component';
export type { AddressState, AddressView } from './security/address.component';
export { KeelSecurityComponent, MODAL_WIDTH } from './security/security.component';
export type { PasswordState } from './security/security.component';
export { KeelTotpEnrollComponent } from './security/enroll.component';
export type { EnrollStage } from './security/enroll.component';
export { KeelTotpDisableComponent } from './security/disable.component';

export const KEEL_SHELL_VERSION = '0.3.0';
