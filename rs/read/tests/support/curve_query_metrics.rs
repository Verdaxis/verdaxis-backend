//! Disposable-only SQLx measurements. Never record SQL text or parameter values.
use std::io::Write;
use std::sync::Mutex;
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Default)]
struct QueryFields {
    rows: Option<u64>,
    summary: String,
    elapsed: f64,
}
impl Visit for QueryFields {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed = value;
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "rows_returned" {
            self.rows = Some(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "summary" {
            self.summary = format!("{value:?}");
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "summary" {
            self.summary = value.to_owned();
        }
    }
}
struct Metrics(Mutex<std::fs::File>);
impl<S: tracing::Subscriber> Layer<S> for Metrics {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut fields = QueryFields::default();
        event.record(&mut fields);
        if let Some(rows) = fields.rows {
            let summary = fields.summary.trim_matches('"').trim();
            let control = summary.starts_with("SET ")
                || summary.starts_with("BEGIN")
                || summary.starts_with("COMMIT");
            let record = serde_json::json!({"rows":rows,"elapsed_secs":fields.elapsed,"control":control,"ping":summary.is_empty()});
            let mut file = self.0.lock().unwrap();
            writeln!(file, "{record}").unwrap();
            file.flush().unwrap();
        }
    }
}
pub fn install() {
    let directory = std::path::Path::new("work/rust-phase1");
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("curve-server.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    let file = std::fs::File::create(directory.join("curve-query-metrics.jsonl")).unwrap();
    tracing_subscriber::registry()
        .with(Metrics(Mutex::new(file)))
        .init();
}
