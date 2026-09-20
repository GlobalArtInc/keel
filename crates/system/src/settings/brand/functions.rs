use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{EntityTrait, NotSet, Set};

use super::super::entity::brand;
use super::{CHROMA_MAX, HUE_MAX, check_svg, normalise_host};

pub const LIST: &str = "settings.brand.list";
pub const GET: &str = "settings.brand.get";
pub const SET: &str = "settings.brand.set";
pub const DELETE: &str = "settings.brand.delete";
pub const LOGO: &str = "settings.brand.logo";
const LOGO_TYPE: &str = "image/svg+xml";

pub const HOST_MAX_LEN: usize = 253;
pub const NAME_MAX_LEN: usize = 120;
pub const LOGO_MAX_LEN: usize = 65536;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct BrandPick {
    #[schemars(length(min = 1, max = HOST_MAX_LEN))]
    pub host: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct BrandSet {
    #[schemars(length(min = 1, max = HOST_MAX_LEN), extend("x-keel" = json!({ "check": "domain" })))]
    pub host: String,
    #[schemars(length(max = NAME_MAX_LEN))]
    pub name: Option<String>,
    #[schemars(range(max = HUE_MAX))]
    pub hue: Option<u16>,
    #[schemars(range(min = 0.0, max = CHROMA_MAX))]
    pub chroma: Option<f64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct BrandLogo {
    #[schemars(length(min = 1, max = HOST_MAX_LEN), extend("x-keel" = json!({ "check": "domain" })))]
    host: String,
    #[schemars(length(min = 1, max = LOGO_MAX_LEN), extend("x-keel" = multipart::file(LOGO_MAX_LEN as u64, [LOGO_TYPE]), "x-ui" = json!({ "control": "file" })))]
    logo: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct BrandDeleted {
    pub deleted: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars")]
struct BrandRow {
    host: String,
    name: Option<String>,
    logo: bool,
    hue: Option<i32>,
    chroma: Option<f64>,
    updated_at: chrono::DateTime<chrono::Utc>,
    updated_by: String,
}

impl From<brand::Model> for BrandRow {
    fn from(row: brand::Model) -> Self {
        Self {
            host: row.host,
            name: row.name,
            logo: row.logo.is_some(),
            hue: row.hue,
            chroma: row.chroma,
            updated_at: row.updated_at,
            updated_by: row.updated_by,
        }
    }
}

struct BrandSource;

#[async_trait]
impl ListSource for BrandSource {
    type Row = BrandRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<BrandRow>> {
        let page = fetch_page(brand::Entity::find(), call, spec).await?;
        Ok(Page {
            data: page.data.into_iter().map(BrandRow::from).collect(),
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<BrandRow>>(LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("host")
        .ui(List::new("settings.brand.list.title")
            .search(["host", "name"])
            .column(Column::new("host", ColumnType::Text).sortable().primary())
            .column(Column::new("name", ColumnType::Text).sortable())
            .column(Column::new("logo", ColumnType::Boolean))
            .column(Column::new("hue", ColumnType::Number).sortable())
            .column(Column::new("chroma", ColumnType::Number))
            .column(Column::new("updatedAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen(SET)
                    .icon("plus"),
            )
            .action(
                Action::new("set", ActionRole::Edit)
                    .screen(SET)
                    .params([("host", "$id")])
                    .icon("pencil"),
            )
            .action(
                Action::new("logo", ActionRole::Upload)
                    .screen(LOGO)
                    .params([("host", "$id")])
                    .icon("image"),
            )
            .action(
                Action::new("delete", ActionRole::Button)
                    .function(DELETE)
                    .params([("host", "$id")])
                    .confirm(),
            )
            .row_actions(["set", "logo", "delete"])
            .empty("settings.brand.empty", Some("create"))
            .invalidate_on(["settings.brand.*"]))
        .default_sort("host", SortDir::Asc)
        .source(BrandSource)
}

pub fn get() -> FunctionDef {
    function::<BrandPick, brand::Model>(GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .handler(|call: Call<BrandPick>| async move {
            brand::Entity::find_by_id(normalise_host(&call.params().host))
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
        })
}

pub fn set() -> FunctionDef {
    function::<BrandSet, brand::Model>(SET)
        .kind(Kind::Form)
        .level(Level::Admin)
        .unowned()
        .key("host")
        .ui(Form::new("settings.brand.set.title")
            .read(GET)
            .page("main", ["host", "name", "hue", "chroma"])
            .field("host")
            .readonly_if("$mode == 'edit'"))
        .handler(|call: Call<BrandSet>| async move {
            let params = call.params();
            let now = call.require::<dyn Clock>()?.now();
            let row = brand::Entity::insert(brand::ActiveModel {
                host: Set(normalise_host(&params.host)),
                name: Set(params.name.clone()),
                logo: NotSet,
                hue: Set(params.hue.map(i32::from)),
                chroma: Set(params.chroma),
                updated_at: Set(now),
                updated_by: Set(call.principal().id.to_string()),
            })
            .on_conflict(
                OnConflict::column(brand::Column::Host)
                    .update_columns([
                        brand::Column::Name,
                        brand::Column::Hue,
                        brand::Column::Chroma,
                        brand::Column::UpdatedAt,
                        brand::Column::UpdatedBy,
                    ])
                    .to_owned(),
            )
            .exec_with_returning(call.tx()?)
            .await?;
            Ok(row)
        })
}

pub fn logo() -> FunctionDef {
    function::<BrandLogo, brand::Model>(LOGO)
        .level(Level::Admin)
        .unowned()
        .key("host")
        .ui(Form::new("settings.brand.logo.title")
            .page("main", ["host", "logo"])
            .field("host")
            .readonly_if("$mode == 'edit'"))
        .upload(|call: Call<BrandLogo>| async move {
            let params = call.params();
            check_svg(&params.logo)?;
            let now = call.require::<dyn Clock>()?.now();
            let row = brand::Entity::insert(brand::ActiveModel {
                host: Set(normalise_host(&params.host)),
                name: NotSet,
                logo: Set(Some(params.logo.clone())),
                hue: NotSet,
                chroma: NotSet,
                updated_at: Set(now),
                updated_by: Set(call.principal().id.to_string()),
            })
            .on_conflict(
                OnConflict::column(brand::Column::Host)
                    .update_columns([
                        brand::Column::Logo,
                        brand::Column::UpdatedAt,
                        brand::Column::UpdatedBy,
                    ])
                    .to_owned(),
            )
            .exec_with_returning(call.tx()?)
            .await?;
            Ok(row)
        })
}

pub fn delete() -> FunctionDef {
    function::<BrandPick, BrandDeleted>(DELETE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(|call: Call<BrandPick>| async move {
            let deleted = brand::Entity::delete_by_id(normalise_host(&call.params().host))
                .exec(call.tx()?)
                .await?;
            if deleted.rows_affected == 0 {
                return Err(KeelError::not_found("keel.error.not_found"));
            }
            Ok(BrandDeleted { deleted: true })
        })
}
