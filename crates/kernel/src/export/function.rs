use std::io::Write;
use std::sync::Arc;

use keel_sdk::list::DEFAULT_MAX_PAGE_SIZE;
use keel_sdk::prelude::*;
use keel_sdk::runtime::TASKS_UNAVAILABLE_KEY;
use keel_sdk::{Download, DownloadOutcome, KeelError, TaskHandle};
use keel_types::ui::Band;
use keel_types::{Descriptor, ExportFormat, Kind, ListQuery, Page};
use serde_json::{Map, Value};

use super::json::Json;
use super::{BAND_KEY, Csv, columns_of, content_type, extension, labels, list_columns};
use crate::dispatcher::{CallerHandle, guards};
use crate::http::Projection;
use crate::registry::Registry;

pub const FUNCTION: &str = "keel.export";
pub const EXPORT_TASK_ROWS: u32 = 10_000;
pub const CLASS: &str = "export";
const OUTPUT_KEY: &str = "keel.error.export_output";
const REPORT_KEY: &str = "keel.error.export_report";

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
struct ExportParams {
    func: String,
    #[serde(default)]
    query: ListQuery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
    #[serde(default)]
    format: ExportFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    band: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    params: Map<String, Value>,
}

struct Target {
    registry: Arc<Registry>,
    descriptor: Descriptor,
    page_size: u32,
}

pub fn function() -> FunctionDef {
    keel_sdk::function::<ExportParams, DownloadFile>(FUNCTION)
        .level(Level::User)
        .unowned()
        .class(CLASS)
        .batch_threshold(EXPORT_TASK_ROWS)
        .download(|call: Call<ExportParams>| async move { export(call).await })
}

async fn export(call: Call<ExportParams>) -> Result<DownloadOutcome> {
    let target = resolve(&call)?;
    if target.descriptor.kind == Kind::Report {
        return export_band(&call, &target).await;
    }
    let params = call.params();
    let columns = columns_of(
        &params.func,
        list_columns(&target.descriptor),
        params.columns.as_deref(),
    )?;
    let mut query = params.query.clone();
    query.page = 1;
    query.page_size = target.page_size;
    let first = fetch(&call, &params.func, &query).await?;
    if first.total > u64::from(EXPORT_TASK_ROWS) && call.task().is_none() {
        return match call.spawn_self_as_task().await {
            Ok(reference) => Ok(reference.into()),
            Err(error) if error.key() == TASKS_UNAVAILABLE_KEY => Err(error
                .param("rows", first.total)
                .param("threshold", EXPORT_TASK_ROWS)),
            Err(error) => Err(error),
        };
    }
    let columns = match (columns.is_empty(), first.data.first()) {
        (true, Some(Value::Object(row))) => row.keys().cloned().collect(),
        _ => columns,
    };
    let labels = labels(
        &target.registry,
        &call.principal().lang,
        &params.func,
        &columns,
    );
    let now = call.require::<dyn Clock>()?.now();
    let date = now.with_timezone(&call.tz()).date_naive();
    let filename = format!("{}-{date}.{}", params.func, extension(params.format));
    let content_type = content_type(params.format);
    let writer = Writer {
        format: params.format,
        columns,
        labels,
        picked: params.columns.is_some(),
    };
    match call.task() {
        Some(task) => {
            let mut output = task.output()?;
            write_all(&call, &writer, query, first, &mut output, Some(task)).await?;
            output.flush().map_err(output_failed)?;
            Ok(DownloadOutcome::stored(filename, content_type))
        }
        None => {
            let mut buffer = Vec::new();
            write_all(&call, &writer, query, first, &mut buffer, None).await?;
            Ok(Download::bytes(filename, content_type, buffer).into())
        }
    }
}

async fn export_band(call: &Call<ExportParams>, target: &Target) -> Result<DownloadOutcome> {
    let params = call.params();
    let band = band_of(&target.descriptor, params.band.as_deref())?;
    let columns = columns_of(&params.func, &band.columns, params.columns.as_deref())?;
    let answer = call
        .invoke(&params.func, Value::Object(params.params.clone()))
        .await?;
    let rows = rows_of(&params.func, &band.id, answer)?;
    let labels = labels(
        &target.registry,
        &call.principal().lang,
        &params.func,
        &columns,
    );
    let now = call.require::<dyn Clock>()?.now();
    let date = now.with_timezone(&call.tz()).date_naive();
    let filename = format!(
        "{}-{}-{date}.{}",
        params.func,
        band.id,
        extension(params.format)
    );
    let writer = Writer {
        format: params.format,
        columns,
        labels,
        picked: params.columns.is_some(),
    };
    let mut buffer = Vec::new();
    let mut json = writer.begin(&mut buffer)?;
    for row in rows {
        buffer.extend_from_slice(writer.line(&mut json, row).as_bytes());
    }
    finish(json, &mut buffer)?;
    Ok(Download::bytes(filename, content_type(params.format), buffer).into())
}

fn band_of<'a>(descriptor: &'a Descriptor, band: Option<&str>) -> Result<&'a Band> {
    let wanted = band.unwrap_or_default();
    descriptor
        .ui
        .as_ref()
        .and_then(|ui| ui.bands.iter().find(|candidate| candidate.id == wanted))
        .ok_or_else(|| {
            KeelError::validation(BAND_KEY)
                .field("band")
                .param("band", wanted)
                .param("function", descriptor.name.as_str())
        })
}

fn rows_of(func: &str, band: &str, answer: Value) -> Result<Vec<Value>> {
    let refused = || {
        KeelError::internal(REPORT_KEY)
            .param("function", func)
            .param("band", band)
    };
    let Value::Object(mut answer) = answer else {
        return Err(refused());
    };
    let Some(Value::Object(mut bands)) = answer.remove("bands") else {
        return Err(refused());
    };
    match bands.remove(band) {
        Some(Value::Array(rows)) => Ok(rows),
        _ => Err(refused()),
    }
}

fn resolve(call: &Call<ExportParams>) -> Result<Target> {
    let func = call.params().func.as_str();
    let not_found =
        || KeelError::not_found("keel.error.function_not_found").param("function", func);
    let registry = call.require::<CallerHandle>()?.registry().ok_or_else(|| {
        KeelError::unavailable("keel.error.caller_unavailable").param("function", FUNCTION)
    })?;
    let function = registry
        .function(func)
        .filter(|function| matches!(function.def().kind(), Kind::List | Kind::Report))
        .ok_or_else(not_found)?;
    let projection = Projection::new(&registry, call.principal());
    projection.admit(function).map_err(|_| not_found())?;
    guards::scope(function.name(), function.def().mode(), call.principal())
        .map_err(|_| not_found())?;
    let descriptor = projection.project(function);
    let def = function.def();
    let page_size = def
        .list_spec()
        .map(|spec| spec.max_page_size)
        .or(def.limits().max_page_size)
        .unwrap_or(DEFAULT_MAX_PAGE_SIZE)
        .max(1);
    Ok(Target {
        registry,
        descriptor,
        page_size,
    })
}

async fn fetch(call: &Call<ExportParams>, func: &str, query: &ListQuery) -> Result<Page<Value>> {
    let page = call.invoke(func, serde_json::to_value(query)?).await?;
    serde_json::from_value(page).map_err(|error| {
        KeelError::internal("keel.error.export_page")
            .param("function", func)
            .source(error)
    })
}

struct Writer {
    format: ExportFormat,
    columns: Vec<String>,
    labels: Vec<String>,
    picked: bool,
}

async fn write_all<W: Write>(
    call: &Call<ExportParams>,
    writer: &Writer,
    mut query: ListQuery,
    first: Page<Value>,
    out: &mut W,
    task: Option<&TaskHandle>,
) -> Result<()> {
    let total = first.total;
    let page_size = query.page_size;
    let mut json = writer.begin(out)?;
    let mut page = first;
    let mut written: u64 = 0;
    loop {
        let count = page.data.len();
        for row in page.data {
            out.write_all(writer.line(&mut json, row).as_bytes())
                .map_err(output_failed)?;
        }
        written += count as u64;
        if let Some(task) = task {
            out.flush().map_err(output_failed)?;
            task.progress(written, Some(total), None).await?;
            if task.is_cancelled() {
                return Err(task.cancelled());
            }
        }
        if count == 0 || count < page_size as usize || written >= total {
            break;
        }
        query.page += 1;
        page = fetch(call, &call.params().func, &query).await?;
    }
    finish(json, out)
}

impl Writer {
    fn begin<W: Write>(&self, out: &mut W) -> Result<Option<Json>> {
        match self.format {
            ExportFormat::Csv => {
                let labels: Vec<&str> = self.labels.iter().map(String::as_str).collect();
                out.write_all(Csv::record(labels).as_bytes())
                    .map_err(output_failed)?;
                Ok(None)
            }
            ExportFormat::Json => {
                let (json, head) = Json::begin();
                out.write_all(head.as_bytes()).map_err(output_failed)?;
                Ok(Some(json))
            }
        }
    }

    fn line(&self, json: &mut Option<Json>, row: Value) -> String {
        match json {
            Some(json) => json.row(self.picked.then_some(self.columns.as_slice()), row),
            None => {
                let cells = Csv::row(&self.columns, &row);
                Csv::record(cells.iter().map(String::as_str))
            }
        }
    }
}

fn finish<W: Write>(json: Option<Json>, out: &mut W) -> Result<()> {
    if let Some(json) = json {
        out.write_all(json.end().as_bytes())
            .map_err(output_failed)?;
    }
    Ok(())
}

fn output_failed(source: std::io::Error) -> KeelError {
    KeelError::internal(OUTPUT_KEY).source(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keel_export_is_a_user_level_download_with_the_row_threshold_declared() {
        let def = function();
        assert_eq!(def.name(), FUNCTION);
        assert_eq!(def.kind(), Kind::Download);
        assert_eq!(def.mode(), Mode::Read);
        assert_eq!(def.access().level, Level::User);
        assert_eq!(def.access().permission, None);
        assert_eq!(def.limits().batch_threshold, Some(EXPORT_TASK_ROWS));
        assert_eq!(def.class(), CLASS);
        assert!(!def.is_resumable());
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).expect("json");
        assert_eq!(descriptor["params"]["required"], json!(["func"]));
        assert_eq!(
            descriptor["params"]["properties"]["format"]["default"],
            json!("csv")
        );
        assert!(descriptor["params"]["$defs"]["ListQuery"].is_object());
        assert_eq!(descriptor["result"]["title"], json!("DownloadFile"));
        assert!(descriptor.get("ui").is_none());
    }
}
