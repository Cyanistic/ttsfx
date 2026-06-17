//! Span display hints for CLI formatting.

use tracing::{span, Subscriber};
use tracing_subscriber::{layer::Context, registry::LookupSpan, Layer};

pub struct SpanDisplayHint {
    pub hidden: bool,
    pub display_name: Option<String>,
}

pub struct SpanTransformLayer;

impl SpanTransformLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SpanTransformLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for SpanTransformLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, _attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).expect("span not found");
        span.extensions_mut().insert(SpanDisplayHint {
            hidden: false,
            display_name: None,
        });
    }
}
