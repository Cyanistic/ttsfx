//! Span display hints for CLI formatting. Optional hide/rename rules.

use std::fmt;

use tracing::{
    field::{Field, Visit},
    span, Subscriber,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

pub struct SpanDisplayHint {
    pub hidden: bool,
    pub display_name: Option<String>,
}

type HideRule = Box<dyn Fn(&str, &str) -> bool + Send + Sync>;
type RenameRule = Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;

pub struct SpanTransformLayer {
    hide_rules: Vec<HideRule>,
    rename_rules: Vec<RenameRule>,
}

impl Default for SpanTransformLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanTransformLayer {
    pub fn new() -> Self {
        Self {
            hide_rules: Vec::new(),
            rename_rules: Vec::new(),
        }
    }

    pub fn hide(mut self, rule: impl Fn(&str, &str) -> bool + Send + Sync + 'static) -> Self {
        self.hide_rules.push(Box::new(rule));
        self
    }

    pub fn rename(
        mut self,
        rule: impl Fn(&str, &str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.rename_rules.push(Box::new(rule));
        self
    }

    fn compute_hint(&self, name: &str, fields: &str) -> SpanDisplayHint {
        let hidden = self.hide_rules.iter().any(|rule| rule(name, fields));

        let display_name = if hidden {
            None
        } else {
            self.rename_rules.iter().find_map(|rule| rule(name, fields))
        };

        SpanDisplayHint {
            hidden,
            display_name,
        }
    }
}

impl<S> Layer<S> for SpanTransformLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let name = span.name();
        let meta = span.metadata();

        let mut visitor = FieldCollector::default();
        attrs.record(&mut visitor);

        let hint = self.compute_hint(name, &visitor.fields);
        let display_name = hint.display_name.clone();
        span.extensions_mut().insert(hint);
        drop(span);

        if let Some(display_name) = display_name
            && let Some(field) = meta.fields().field("otel.name") {
                tracing::dispatcher::get_default(|dispatch| {
                    let name_str = display_name.as_str();
                    let values = [(
                        &field,
                        Some(&name_str as &dyn tracing::field::Value),
                    )];
                    let value_set = meta.fields().value_set(&values);
                    dispatch.record(id, &span::Record::new(&value_set));
                });
            }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        let name = span.name();

        let mut visitor = FieldCollector::default();
        values.record(&mut visitor);

        let existing_fields = {
            let ext = span.extensions();
            ext.get::<CollectedFields>()
                .map(|f| f.0.clone())
                .unwrap_or_default()
        };

        let all_fields = if existing_fields.is_empty() {
            visitor.fields.clone()
        } else if visitor.fields.is_empty() {
            existing_fields
        } else {
            format!("{existing_fields} {}", visitor.fields)
        };

        let hint = self.compute_hint(name, &all_fields);
        let mut ext = span.extensions_mut();
        ext.replace(hint);
        ext.replace(CollectedFields(all_fields));
    }
}

struct CollectedFields(String);

#[derive(Default)]
struct FieldCollector {
    fields: String,
}

impl FieldCollector {
    fn push_separator(&mut self) {
        if !self.fields.is_empty() {
            self.fields.push(' ');
        }
    }
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push_separator();
        self.fields
            .push_str(&format!("{}={:?}", field.name(), value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_separator();
        self.fields.push_str(&format!("{}={}", field.name(), value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push_separator();
        self.fields.push_str(&format!("{}={}", field.name(), value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push_separator();
        self.fields.push_str(&format!("{}={}", field.name(), value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push_separator();
        self.fields.push_str(&format!("{}={}", field.name(), value));
    }
}