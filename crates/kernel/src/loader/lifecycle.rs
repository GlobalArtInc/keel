use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use keel_sdk::runtime::{Spawner, StopHook};
use keel_sdk::{Context, Db, KeelError};
use sea_orm::DatabaseConnection;
use tokio::task::JoinSet;

use super::composition::Graph;
use super::freeze::Frozen;
use crate::backup::Site;
use crate::registry::Registry;

pub struct Kernel {
    graph: Graph,
    registry: Arc<Registry>,
    running: Vec<Running>,
}

struct Running {
    module: String,
    stop: Option<StopHook>,
    tasks: Arc<TaskSet>,
}

struct TaskSet {
    module: String,
    tasks: Mutex<Option<JoinSet<()>>>,
}

#[derive(Debug, thiserror::Error)]
#[error("module {module}: start failed: {source}")]
pub struct StartError {
    pub module: String,
    #[source]
    pub source: KeelError,
}

#[derive(Debug, thiserror::Error)]
#[error("module {module}: attach failed: {source}")]
pub struct AttachError {
    pub module: String,
    #[source]
    pub source: KeelError,
}

#[derive(Debug, thiserror::Error)]
pub struct StopError {
    failed: Vec<(String, KeelError)>,
}

impl Frozen {
    pub async fn start(self, db: DatabaseConnection) -> Result<Kernel, StartError> {
        self.locks.attach(db.clone());
        self.backups
            .attach(Arc::new(Site::new(&self.graph, &self.registry)), db.clone());
        let data_dir = self.graph.data_dir().to_path_buf();
        let mut kernel = Kernel {
            graph: self.graph,
            registry: self.registry,
            running: Vec::with_capacity(self.lifecycle.len()),
        };
        for lifecycle in self.lifecycle {
            let tasks = Arc::new(TaskSet::new(&lifecycle.module));
            let ctx = Context::new(
                &lifecycle.module,
                kernel.registry.contributions().clone(),
                kernel.registry.services().clone(),
                tasks.clone(),
                Db::from_connection(db.clone()),
                data_dir.clone(),
            );
            for hook in [lifecycle.attach, lifecycle.start].into_iter().flatten() {
                if let Err(source) = hook(ctx.clone()).await {
                    tasks.shutdown().await;
                    let failed = StartError {
                        module: lifecycle.module,
                        source,
                    };
                    tracing::error!(error = %failed, "start failed; stopping what started");
                    if let Err(error) = kernel.stop().await {
                        tracing::error!(error = %error, "stopping after a failed start");
                    }
                    return Err(failed);
                }
            }
            tracing::debug!(module = lifecycle.module, "module started");
            kernel.running.push(Running {
                module: lifecycle.module,
                stop: lifecycle.stop,
                tasks,
            });
        }
        tracing::info!(modules = ?kernel.graph.ids(), "modules started");
        Ok(kernel)
    }
}

impl Frozen {
    pub async fn attach(&mut self, db: DatabaseConnection) -> Result<(), AttachError> {
        let data_dir = self.graph.data_dir().to_path_buf();
        for lifecycle in &mut self.lifecycle {
            let Some(attach) = lifecycle.attach.take() else {
                continue;
            };
            let ctx = Context::new(
                &lifecycle.module,
                self.registry.contributions().clone(),
                self.registry.services().clone(),
                Arc::new(Detached {
                    module: lifecycle.module.clone(),
                }),
                Db::from_connection(db.clone()),
                data_dir.clone(),
            );
            attach(ctx).await.map_err(|source| AttachError {
                module: lifecycle.module.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

struct Detached {
    module: String,
}

impl Spawner for Detached {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        drop(task);
        tracing::error!(
            module = self.module,
            "a task was spawned by an attach hook run without the module starting; it is dropped"
        );
    }
}

impl Kernel {
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    pub fn running(&self) -> impl Iterator<Item = &str> {
        self.running.iter().map(|running| running.module.as_str())
    }

    pub async fn stop(mut self) -> Result<(), StopError> {
        let mut failed = Vec::new();
        while let Some(running) = self.running.pop() {
            if let Some(stop) = running.stop
                && let Err(error) = stop().await
            {
                tracing::error!(module = running.module, error = %error, "stop hook failed");
                failed.push((running.module.clone(), error));
            }
            running.tasks.shutdown().await;
            tracing::debug!(module = running.module, "module stopped");
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(StopError { failed })
        }
    }
}

impl TaskSet {
    fn new(module: &str) -> Self {
        Self {
            module: module.to_owned(),
            tasks: Mutex::new(Some(JoinSet::new())),
        }
    }

    async fn shutdown(&self) {
        let taken = self
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut tasks) = taken else {
            return;
        };
        tasks.abort_all();
        while let Some(outcome) = tasks.join_next().await {
            if let Err(error) = outcome
                && error.is_panic()
            {
                tracing::error!(module = self.module, "a background task panicked");
            }
        }
    }
}

impl Spawner for TaskSet {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        let mut guard = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.as_mut() {
            Some(tasks) => {
                tasks.spawn(task);
            }
            None => tracing::warn!(
                module = self.module,
                "a task was spawned after the module stopped; it is dropped"
            ),
        }
    }
}

impl fmt::Debug for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel")
            .field("graph", &self.graph)
            .field("running", &self.running().collect::<Vec<_>>())
            .finish()
    }
}

impl StopError {
    pub fn failed(&self) -> &[(String, KeelError)] {
        &self.failed
    }
}

impl fmt::Display for StopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stop hooks failed:")?;
        for (module, error) in &self.failed {
            write!(f, "\n  - module {module}: {error}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use keel_sdk::Registrar;
    use keel_sdk::prelude::*;

    use super::*;
    use crate::loader::Composition;
    use crate::loader::testing::module;

    static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn note(event: String) {
        EVENTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }

    fn events_of(prefix: &str) -> Vec<String> {
        EVENTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.starts_with(prefix))
            .cloned()
            .collect()
    }

    struct Dropped(String);

    impl Drop for Dropped {
        fn drop(&mut self) {
            note(format!("{}:task_dropped", self.0));
        }
    }

    trait Clock: Send + Sync {
        fn now(&self) -> u64;
    }

    struct Fixed;

    impl Clock for Fixed {
        fn now(&self) -> u64 {
            7
        }
    }

    struct Widget(&'static str);

    fn lifecycle(r: &mut Registrar) -> Result<()> {
        let id = r.module().to_owned();
        let stopped = id.clone();
        r.start(move |ctx| async move {
            note(format!("{id}:start"));
            let guard = Dropped(ctx.module().to_owned());
            ctx.spawn(async move {
                let _guard = guard;
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            });
            Ok(())
        })
        .stop(move || async move {
            note(format!("{stopped}:stop"));
            Ok(())
        });
        Ok(())
    }

    module!(ORDER_A, "order_a", [], lifecycle);
    module!(ORDER_B, "order_b", ["order_a"], lifecycle);
    module!(ORDER_C, "order_c", ["order_b"], lifecycle);

    async fn start(modules: &[&'static keel_sdk::ModuleDef]) -> Result<Kernel, StartError> {
        let mut composition = Composition::new();
        for module in modules {
            composition = composition.module(module);
        }
        composition
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect("the registries freeze")
            .start(DatabaseConnection::Disconnected)
            .await
    }

    #[tokio::test]
    async fn modules_start_in_graph_order_and_stop_in_reverse_cancelling_their_tasks() {
        let kernel = start(&[&ORDER_C, &ORDER_A, &ORDER_B]).await.unwrap();
        assert_eq!(
            kernel.running().collect::<Vec<_>>(),
            ["keel", "order_a", "order_b", "order_c"]
        );
        assert_eq!(
            events_of("order_"),
            ["order_a:start", "order_b:start", "order_c:start"]
        );
        assert!(
            format!("{kernel:?}")
                .contains("running: [\"keel\", \"order_a\", \"order_b\", \"order_c\"]")
        );
        kernel.stop().await.unwrap();
        assert_eq!(
            events_of("order_"),
            [
                "order_a:start",
                "order_b:start",
                "order_c:start",
                "order_c:stop",
                "order_c:task_dropped",
                "order_b:stop",
                "order_b:task_dropped",
                "order_a:stop",
                "order_a:task_dropped",
            ]
        );
    }

    fn attaching(r: &mut Registrar) -> Result<()> {
        let id = r.module().to_owned();
        let attached = id.clone();
        let spawned = id.clone();
        r.attach(move |ctx| async move {
            note(format!("{attached}:attach"));
            ctx.spawn(async move { note(format!("{spawned}:task_ran")) });
            Ok(())
        })
        .start(move |_| async move {
            note(format!("{id}:start"));
            Ok(())
        });
        Ok(())
    }

    module!(ATTACH_A, "attach_a", [], attaching);
    module!(ATTACH_B, "attach_b", ["attach_a"], attaching);

    #[tokio::test]
    async fn attach_hooks_run_in_graph_order_without_starting_and_only_once_before_a_later_start() {
        let mut frozen = Composition::new()
            .module(&ATTACH_B)
            .module(&ATTACH_A)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect("the registries freeze");
        frozen
            .attach(DatabaseConnection::Disconnected)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert_eq!(events_of("attach_"), ["attach_a:attach", "attach_b:attach"]);
        frozen
            .attach(DatabaseConnection::Disconnected)
            .await
            .unwrap();
        let kernel = frozen
            .start(DatabaseConnection::Disconnected)
            .await
            .unwrap();
        assert_eq!(
            events_of("attach_"),
            [
                "attach_a:attach",
                "attach_b:attach",
                "attach_a:start",
                "attach_b:start"
            ]
        );
        kernel.stop().await.unwrap();
    }

    fn plain(r: &mut Registrar) -> Result<()> {
        let id = r.module().to_owned();
        let started = id.clone();
        let stopped = id.clone();
        r.attach(move |_| async move {
            note(format!("{id}:attach"));
            Ok(())
        })
        .start(move |_| async move {
            note(format!("{started}:start"));
            Ok(())
        })
        .stop(move || async move {
            note(format!("{stopped}:stop"));
            Ok(())
        });
        Ok(())
    }

    fn failing_attach(r: &mut Registrar) -> Result<()> {
        r.attach(|ctx| async move {
            note(format!("{}:attach", ctx.module()));
            Err(KeelError::unavailable("roll_b.error.no_socket"))
        });
        Ok(())
    }

    fn failing_attach_outside(r: &mut Registrar) -> Result<()> {
        r.attach(|ctx| async move {
            note(format!("{}:attach", ctx.module()));
            Err(KeelError::unavailable("only_b.error.no_socket"))
        });
        Ok(())
    }

    module!(SERVE_A, "serve_a", [], plain);
    module!(SERVE_B, "serve_b", ["serve_a"], plain);
    module!(ROLL_A, "roll_a", [], plain);
    module!(ROLL_B, "roll_b", ["roll_a"], failing_attach);
    module!(ONLY_A, "only_a", [], plain);
    module!(ONLY_B, "only_b", ["only_a"], failing_attach_outside);

    #[tokio::test]
    async fn a_start_runs_the_attach_hook_of_every_module_right_before_its_own_start_hook() {
        let kernel = start(&[&SERVE_B, &SERVE_A]).await.unwrap();
        assert_eq!(
            events_of("serve_"),
            [
                "serve_a:attach",
                "serve_a:start",
                "serve_b:attach",
                "serve_b:start"
            ]
        );
        kernel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn a_failing_attach_hook_during_a_start_stops_the_modules_that_started_and_names_the_module()
     {
        let error = start(&[&ROLL_A, &ROLL_B]).await.unwrap_err();
        assert_eq!(error.module, "roll_b");
        assert_eq!(
            error.to_string(),
            "module roll_b: start failed: unavailable: roll_b.error.no_socket"
        );
        assert_eq!(
            events_of("roll_"),
            [
                "roll_a:attach",
                "roll_a:start",
                "roll_b:attach",
                "roll_a:stop"
            ]
        );
    }

    #[tokio::test]
    async fn a_failing_attach_hook_outside_a_start_is_named_as_an_attach_failure() {
        let mut frozen = Composition::new()
            .module(&ONLY_B)
            .module(&ONLY_A)
            .resolve()
            .expect("the composition resolves")
            .init()
            .expect("init registers")
            .freeze()
            .expect("the registries freeze");
        let error = frozen
            .attach(DatabaseConnection::Disconnected)
            .await
            .unwrap_err();
        assert_eq!(error.module, "only_b");
        assert_eq!(
            error.to_string(),
            "module only_b: attach failed: unavailable: only_b.error.no_socket"
        );
    }

    fn failing_start(r: &mut Registrar) -> Result<()> {
        r.start(|ctx| async move {
            note(format!("{}:start", ctx.module()));
            Err(KeelError::unavailable("fail_b.error.no_socket"))
        })
        .stop(|| async {
            note("fail_b:stop".to_owned());
            Ok(())
        });
        Ok(())
    }

    module!(FAIL_A, "fail_a", [], lifecycle);
    module!(FAIL_B, "fail_b", ["fail_a"], failing_start);
    module!(FAIL_C, "fail_c", ["fail_b"], lifecycle);

    #[tokio::test]
    async fn a_failed_start_stops_the_modules_that_already_started_and_names_the_module() {
        let error = start(&[&FAIL_A, &FAIL_B, &FAIL_C]).await.unwrap_err();
        assert_eq!(error.module, "fail_b");
        assert_eq!(
            error.to_string(),
            "module fail_b: start failed: unavailable: fail_b.error.no_socket"
        );
        assert_eq!(
            events_of("fail_"),
            [
                "fail_a:start",
                "fail_b:start",
                "fail_a:stop",
                "fail_a:task_dropped"
            ]
        );
    }

    fn failing_stop(r: &mut Registrar) -> Result<()> {
        r.stop(|| async { Err(KeelError::internal("stop_b.error.stuck")) });
        Ok(())
    }

    module!(STOP_A, "stop_a", [], lifecycle);
    module!(STOP_B, "stop_b", ["stop_a"], failing_stop);

    #[tokio::test]
    async fn a_failing_stop_hook_does_not_keep_the_other_modules_from_stopping() {
        let kernel = start(&[&STOP_A, &STOP_B]).await.unwrap();
        let error = kernel.stop().await.unwrap_err();
        assert_eq!(error.failed().len(), 1);
        assert_eq!(error.failed()[0].0, "stop_b");
        assert_eq!(
            error.to_string(),
            "stop hooks failed:\n  - module stop_b: internal: stop_b.error.stuck"
        );
        assert_eq!(
            events_of("stop_"),
            ["stop_a:start", "stop_a:stop", "stop_a:task_dropped"]
        );
    }

    fn providing(r: &mut Registrar) -> Result<()> {
        r.provide::<dyn Clock>(Arc::new(Fixed))
            .contribute(Widget("one"))
            .contribute(Widget("two"));
        Ok(())
    }

    fn consuming(r: &mut Registrar) -> Result<()> {
        r.require::<dyn Clock>().start(|ctx| async move {
            let widgets: Vec<&str> = ctx.contributions::<Widget>().map(|w| w.0).collect();
            note(format!("ctx_b:widgets={}", widgets.join(",")));
            note(format!("ctx_b:now={}", ctx.require::<dyn Clock>()?.now()));
            Ok(())
        });
        Ok(())
    }

    module!(CTX_A, "ctx_a", [], providing);
    module!(CTX_B, "ctx_b", ["ctx_a"], consuming);

    #[tokio::test]
    async fn the_start_context_reads_the_contribution_bag_and_the_services() {
        let kernel = start(&[&CTX_B, &CTX_A]).await.unwrap();
        assert_eq!(events_of("ctx_"), ["ctx_b:widgets=one,two", "ctx_b:now=7"]);
        kernel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn a_task_spawned_after_stop_is_dropped_instead_of_leaking() {
        let set = TaskSet::new("late");
        set.spawn(Box::pin(async {}));
        set.shutdown().await;
        set.spawn(Box::pin(async {}));
        set.shutdown().await;
        assert!(set.tasks.lock().unwrap().is_none());
    }
}
