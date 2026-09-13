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
pub mod ui;

pub use boot::{
    Badge, Boot, BootBrand, BootMenuGroup, BootMenuItem, BootPrincipal, BootProduct, Platform,
};
pub use check::Check;
pub use descriptor::{
    Access, Control, DESCRIPTOR_VERSION, Descriptor, KeelAttrs, Kind, MemberAccess, Mode, UiAttrs,
    X_KEEL, X_UI,
};
pub use envelope::{CallError, Envelope, ErrorCode, FieldError, Meta, MetaActor, MetaPrincipal};
pub use event::{Event, EventActor, EventActorKind, EventSubject};
pub use expr::{Context, Expr};
pub use id::Id;
pub use level::Level;
pub use list::{Filter, FilterOp, FilterValue, ListQuery, Page, Sort, SortDir};
pub use options::{Choice, Choices, OptionsQuery};
pub use principal::{
    Account, AccountState, ActorVia, NotDelegated, Principal, Scope, Via, is_mask,
};
pub use ui::{
    Action, ActionRole, Column, ColumnFilter, ColumnType, Confirm, ConfirmDetails, ConfirmInput,
    Empty, ExportFormat, FieldUi, InlineEdit, Link, ScreenPage, ScreenResult, ScreenUi, Tone,
    Total,
};

pub const PROTOCOL: u32 = 1;

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}
