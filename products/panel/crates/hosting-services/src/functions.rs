use std::cmp::Ordering;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, SecondsFormat, Utc};
use keel::types::{Filter, FilterOp, FilterValue};
use keel_host::api::{HostExecutor, Origin, ServiceStatus};

use crate::PRODUCT;
use crate::watch::{ACTIVE, FAILED, INACTIVE};

pub(crate) const LIST: &str = "hosting.service.list";

const START: &str = "hosting.service.start";
const STOP: &str = "hosting.service.stop";
const RESTART: &str = "hosting.service.restart";
const RELOAD: &str = "hosting.service.reload";
const NAME: &str = "name";
const STATE: &str = "state";
const INSTALLED: &str = "installed";
const MEMORY: &str = "memoryMib";
const SINCE: &str = "since";
const MIB: u64 = 1024 * 1024;
const ACTIVATING: &str = "activating";
const DEACTIVATING: &str = "deactivating";
const RELOADING: &str = "reloading";

#[derive(Clone, Copy)]
enum Verb {
    Start,
    Stop,
    Restart,
    Reload,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    #[schemars(length(min = 1, max = 256))]
    name: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Done {
    name: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "camelCase")]
struct ServiceRow {
    name: String,
    state: String,
    installed: bool,
    memory_mib: Option<u64>,
    since: Option<DateTime<Utc>>,
}

impl ServiceRow {
    fn of(status: ServiceStatus) -> Self {
        Self {
            memory_mib: status.memory.map(|bytes| bytes.div_ceil(MIB)),
            since: status
                .since
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
            name: status.name,
            state: status.state,
            installed: status.installed,
        }
    }

    fn cell(&self, key: &str) -> Option<String> {
        match key {
            NAME => Some(self.name.clone()),
            STATE => Some(self.state.clone()),
            INSTALLED => Some(self.installed.to_string()),
            MEMORY => self.memory_mib.map(|mib| mib.to_string()),
            SINCE => self
                .since
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            _ => None,
        }
    }

    fn matches(&self, filter: &Filter) -> bool {
        let Some(actual) = self.cell(&filter.key) else {
            return filter.op == FilterOp::Ne;
        };
        let compare = |expected: &str| match filter.key.as_str() {
            MEMORY => actual
                .parse::<u64>()
                .unwrap_or_default()
                .cmp(&expected.parse::<u64>().unwrap_or_default()),
            _ => actual.as_str().cmp(expected),
        };
        match &filter.value {
            FilterValue::Many(expected) => {
                let any = expected
                    .iter()
                    .any(|value| compare(value) == Ordering::Equal);
                if filter.op == FilterOp::Ne { !any } else { any }
            }
            FilterValue::One(expected) => {
                let ordering = compare(expected);
                match filter.op {
                    FilterOp::Eq => ordering == Ordering::Equal,
                    FilterOp::Ne => ordering != Ordering::Equal,
                    FilterOp::Gt => ordering == Ordering::Greater,
                    FilterOp::Gte => ordering != Ordering::Less,
                    FilterOp::Lt => ordering == Ordering::Less,
                    FilterOp::Lte => ordering != Ordering::Greater,
                }
            }
        }
    }

    fn order(&self, other: &Self, key: &str) -> Ordering {
        match key {
            STATE => self.state.cmp(&other.state),
            MEMORY => self.memory_mib.cmp(&other.memory_mib),
            SINCE => self.since.cmp(&other.since),
            _ => Ordering::Equal,
        }
        .then_with(|| self.name.cmp(&other.name))
    }
}

struct ServiceSource;

#[async_trait]
impl ListSource for ServiceSource {
    type Row = ServiceRow;

    async fn fetch(&self, call: &Call<ListQuery>, _: &ListSpec) -> Result<Page<ServiceRow>> {
        let host = call.require::<dyn HostExecutor>()?;
        let statuses = host
            .on(Origin::new(PRODUCT, call.principal(), call.request_id()))
            .services()
            .status()
            .await?;
        let query = call.params();
        let search = query.search.trim().to_lowercase();
        let mut rows: Vec<ServiceRow> = statuses
            .into_iter()
            .map(ServiceRow::of)
            .filter(|row| search.is_empty() || row.name.to_lowercase().contains(&search))
            .filter(|row| query.filters.iter().all(|filter| row.matches(filter)))
            .collect();
        let (key, dir) = query
            .sort
            .as_ref()
            .map_or((NAME, SortDir::Asc), |sort| (sort.key.as_str(), sort.dir));
        rows.sort_by(|left, right| match dir {
            SortDir::Asc => left.order(right, key),
            SortDir::Desc => left.order(right, key).reverse(),
        });
        let total = rows.len() as u64;
        let size = query.page_size as usize;
        let start = (query.page as usize - 1).saturating_mul(size);
        let data = rows.into_iter().skip(start).take(size).collect();
        Ok(Page::new(data, total))
    }
}

fn on_row(id: &str, function: &str, icon: &str) -> Action {
    Action::new(id, ActionRole::Button)
        .function(function)
        .params([(NAME, "$id")])
        .icon(icon)
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<ServiceRow>>(LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key(NAME)
        .ui(List::new("hosting.service.list.title")
            .search([NAME])
            .column(Column::new(NAME, ColumnType::Text).sortable().primary())
            .column(
                Column::new(STATE, ColumnType::Status)
                    .sortable()
                    .map([
                        (ACTIVE, Tone::Success),
                        (RELOADING, Tone::Info),
                        (ACTIVATING, Tone::Info),
                        (DEACTIVATING, Tone::Warning),
                        (INACTIVE, Tone::Neutral),
                        (FAILED, Tone::Danger),
                    ])
                    .filter_options([ACTIVE, INACTIVE, FAILED]),
            )
            .column(Column::new(INSTALLED, ColumnType::Boolean))
            .column(Column::new(MEMORY, ColumnType::Number).sortable())
            .column(Column::new(SINCE, ColumnType::Datetime).sortable())
            .action(on_row("start", START, "play"))
            .action(on_row("stop", STOP, "square").confirm_with(ConfirmDetails {
                destructive: true,
                items: Some(NAME.to_owned()),
                input: None,
            }))
            .action(on_row("restart", RESTART, "rotate-cw").confirm())
            .action(on_row("reload", RELOAD, "refresh-cw"))
            .action(Action::new("refresh", ActionRole::Refresh))
            .row_actions(["start", "stop", "restart", "reload"])
            .empty("hosting.service.empty", None)
            .invalidate_on(["hosting.service.*"]))
        .default_sort(NAME, SortDir::Asc)
        .source(ServiceSource)
}

pub(crate) fn start() -> FunctionDef {
    act(START, Verb::Start)
}

pub(crate) fn stop() -> FunctionDef {
    act(STOP, Verb::Stop)
}

pub(crate) fn restart() -> FunctionDef {
    act(RESTART, Verb::Restart)
}

pub(crate) fn reload() -> FunctionDef {
    act(RELOAD, Verb::Reload)
}

fn act(name: &str, verb: Verb) -> FunctionDef {
    function::<Pick, Done>(name)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .key(NAME)
        .handler(move |call: Call<Pick>| async move {
            let host = call.require::<dyn HostExecutor>()?;
            let host = host.on(Origin::new(PRODUCT, call.principal(), call.request_id()));
            let services = host.services();
            let unit = call.params().name.as_str();
            match verb {
                Verb::Start => services.start(unit, None).await?,
                Verb::Stop => services.stop(unit).await?,
                Verb::Restart => services.restart(unit, None).await?,
                Verb::Reload => services.reload(unit, None).await?,
            }
            Ok(Done {
                name: unit.to_owned(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, state: &str, memory: Option<u64>) -> ServiceRow {
        ServiceRow::of(ServiceStatus {
            name: name.to_owned(),
            state: state.to_owned(),
            installed: true,
            memory,
            since: memory.map(|_| 1_790_000_000),
        })
    }

    fn filter(value: Value) -> Filter {
        keel::sdk::serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_row_rounds_memory_up_to_mebibytes_and_reads_its_start_as_an_instant() {
        let nginx = row("nginx", ACTIVE, Some(MIB + 1));
        assert_eq!(nginx.memory_mib, Some(2));
        assert_eq!(nginx.cell(SINCE).as_deref(), Some("2026-09-21T14:13:20Z"));
        assert_eq!(row("postfix", INACTIVE, None).cell(MEMORY), None);
    }

    #[test]
    fn filters_compare_states_as_text_and_memory_as_a_number() {
        let nginx = row("nginx", ACTIVE, Some(12 * MIB));
        let postfix = row("postfix", FAILED, None);
        let failed = filter(json!({ "key": STATE, "op": "eq", "value": [FAILED] }));
        assert!(!nginx.matches(&failed));
        assert!(postfix.matches(&failed));
        let large = filter(json!({ "key": MEMORY, "op": "gt", "value": "9" }));
        assert!(nginx.matches(&large));
        assert!(!postfix.matches(&large));
        let not_failed = filter(json!({ "key": STATE, "op": "ne", "value": [FAILED] }));
        assert!(nginx.matches(&not_failed));
        assert!(!postfix.matches(&not_failed));
    }

    #[test]
    fn the_actions_are_admin_writes_and_the_list_an_admin_read() {
        for def in [start(), stop(), restart(), reload()] {
            assert_eq!(def.kind(), Kind::Action, "{}", def.name());
            assert_eq!(def.mode(), Mode::Write, "{}", def.name());
            assert_eq!(def.access().level, Level::Admin, "{}", def.name());
        }
        let list = list();
        assert_eq!(list.mode(), Mode::Read);
        assert_eq!(list.access().level, Level::Admin);
    }
}
