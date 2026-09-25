pub mod boot;
pub mod check;
pub mod descriptor;
pub mod envelope;
pub mod event;
pub mod expr;
pub mod id;
pub mod level;
pub mod list;
pub mod options;
pub mod principal;
pub mod task;
pub mod ui;

pub use boot::{
    Badge, Boot, BootBrand, BootLicense, BootMenuGroup, BootMenuItem, BootPrincipal, BootProblems,
    BootProduct, BootWidget, DashboardLayout, LicenseState, Platform, StatValue, WidgetKind,
};
pub use check::Check;
pub use descriptor::{
    Access, CheckBy, Control, DESCRIPTOR_VERSION, Descriptor, FileAttrs, KeelAttrs, Kind,
    MemberAccess, Mode, UiAttrs, X_KEEL, X_UI,
};
pub use envelope::{CallError, Envelope, ErrorCode, FieldError, Meta, MetaActor, MetaPrincipal};
pub use event::{
    Event, EventActor, EventActorKind, EventSubject, NotificationFrame, NotificationLevel,
    UnknownNotificationLevel,
};
pub use expr::{Context, Expr};
pub use id::Id;
pub use level::Level;
pub use list::{Filter, FilterOp, FilterValue, ListQuery, Page, Sort, SortDir};
pub use options::{Choice, Choices, OptionsQuery};
pub use principal::{
    Account, AccountState, ActorVia, NotDelegated, Principal, Scope, Via, is_mask,
};
pub use task::{
    DOWNLOAD_FUNCTION, DownloadFile, DownloadLink, DownloadRef, GroupResult, TaskLink,
    TaskProgress, TaskRef, TaskState, UnknownTaskState,
};
pub use ui::{
    Action, ActionRole, ChartType, Column, ColumnFilter, ColumnType, Confirm, ConfirmDetails,
    ConfirmInput, Empty, ExportFormat, FieldUi, InlineEdit, Layout, Link, ScreenPage, ScreenResult,
    ScreenUi, Tone, Total,
};

pub const PROTOCOL: u32 = 1;

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}
