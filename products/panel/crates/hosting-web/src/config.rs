use keel::prelude::*;
use keel::sdk::sea_orm::{ActiveModelTrait, Set};
use keel::sdk::serde_json;
use keel_host::api::Origin;

use crate::functions::{Pick, apply_after_commit, load};
use crate::host::{Effects, Turn};
use crate::{FEATURE, PRODUCT, site};

pub(crate) const VIEW: &str = "hosting.site.config-editor";
pub(crate) const SET: &str = "hosting.site.config.set";
pub(crate) const SERVER: &str = "server";
pub(crate) const POOL: &str = "pool";
const GET: &str = "hosting.site.config.get";
const TEXT: &str = "hosting.site.config.error.text";
const CONFIG_MAX: usize = 16_384;

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct SiteConfigSet {
    id: Id,
    #[serde(default)]
    #[schemars(length(max = CONFIG_MAX), extend("x-ui" = json!({ "control": "code" })))]
    server: String,
    #[serde(default)]
    #[schemars(length(max = CONFIG_MAX), extend("x-ui" = json!({ "control": "code" })))]
    pool: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct SiteConfig {
    id: String,
    domain: String,
    php: Option<String>,
    server: String,
    pool: String,
    rendered: RenderedConfig,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct RenderedConfig {
    server: Option<String>,
    pool: Option<String>,
}

pub(crate) fn get(turn: Turn) -> FunctionDef {
    function::<Pick, SiteConfig>(GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .handler(move |call: Call<Pick>| {
            let turn = Arc::clone(&turn);
            async move {
                let row = load(call.db(), &call.params().id).await?;
                let installed = Effects::of(&call, &turn)?
                    .installed(origin(&call), &row)
                    .await?;
                Ok(SiteConfig {
                    id: row.id,
                    domain: row.domain,
                    php: row.php,
                    server: row.server_config,
                    pool: row.pool_config,
                    rendered: RenderedConfig {
                        server: installed.server,
                        pool: installed.pool,
                    },
                })
            }
        })
}

pub(crate) fn set(turn: Turn) -> FunctionDef {
    function::<SiteConfigSet, SiteConfigSet>(SET)
        .kind(Kind::Form)
        .level(Level::Admin)
        .feature(FEATURE)
        .owned::<site::Entity>()
        .ui(Form::new("hosting.site.config.set.title")
            .read(GET)
            .page("main", [SERVER, POOL]))
        .handler(move |call: Call<SiteConfigSet>| {
            let turn = Arc::clone(&turn);
            async move {
                let params = call.params();
                plain(params)?;
                let tx = call.tx()?;
                let current = load(tx, &params.id).await?;
                let now = call.require::<dyn Clock>()?.now();
                site::ActiveModel {
                    id: Set(current.id.clone()),
                    server_config: Set(params.server.clone()),
                    pool_config: Set(params.pool.clone()),
                    state: Set(site::STATE_PENDING.to_owned()),
                    error: Set(None),
                    updated_at: Set(now),
                    ..Default::default()
                }
                .update(tx)
                .await?;
                apply_after_commit(&call, &turn, vec![current.id])?;
                Ok(params.clone())
            }
        })
}

pub(crate) async fn trial(call: Call<Value>, turn: Turn) -> Result<Before> {
    let params: SiteConfigSet = serde_json::from_value(call.params().clone())?;
    plain(&params)?;
    let stored = load(call.db(), &params.id).await?;
    let candidate = site::Model {
        server_config: params.server,
        pool_config: params.pool,
        ..stored.clone()
    };
    Effects::of(&call, &turn)?
        .trial(call.db(), origin(&call), &stored, &candidate)
        .await?;
    Ok(Before::Proceed)
}

pub(crate) fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn plain(params: &SiteConfigSet) -> Result<()> {
    for (field, text) in [(SERVER, &params.server), (POOL, &params.pool)] {
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(KeelError::validation(TEXT).field(field));
        }
    }
    Ok(())
}

fn origin<P>(call: &Call<P>) -> Origin {
    Origin::new(PRODUCT, call.principal(), call.request_id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_of_a_configuration_drop_trailing_blanks_and_carriage_returns() {
        assert_eq!(
            lines("client_max_body_size 64m;  \r\n\r\nlocation /x {\n}\n\n  \n"),
            ["client_max_body_size 64m;", "", "location /x {", "}"]
        );
        assert!(lines("").is_empty());
    }

    #[test]
    fn a_control_character_other_than_a_line_break_or_a_tab_is_refused_on_its_field() {
        let set = |server: &str, pool: &str| SiteConfigSet {
            id: Id::generate(),
            server: server.to_owned(),
            pool: pool.to_owned(),
        };
        assert!(plain(&set("gzip on;\r\n\tgzip_types text/css;\n", "")).is_ok());
        let error = plain(&set("", "pm = static\u{1b}")).expect_err("an escape in the pool");
        assert_eq!(error.field_path(), Some(POOL));
        assert_eq!(error.key(), TEXT);
    }
}
